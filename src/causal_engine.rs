use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::time::Duration;

use crate::telemetry;
use crate::utils;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum NodeType {
    Process,
    Resource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EdgeType {
    SpawnedBy,
    ReadsFrom,
    WritesTo,
    BlockedOn,
    ContendsWith,
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeType::SpawnedBy => "SPAWNED_BY",
            EdgeType::ReadsFrom => "READS_FROM",
            EdgeType::WritesTo => "WRITES_TO",
            EdgeType::BlockedOn => "BLOCKED_ON",
            EdgeType::ContendsWith => "CONTENDS_WITH",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: String,
    pub node_type: NodeType,
    pub name: String,
    pub pid: i32,
    pub state: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_rate_kb: f64,
    pub write_rate_kb: f64,
    pub cpu_usage_pct: f64,
    pub is_anomalous: bool,
    pub anomaly_reason: String,
}

#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from_id: String,
    pub to_id: String,
    pub edge_type: EdgeType,
    pub details: String,
}

// ── CausalGraph ───────────────────────────────────────────────────────────────

pub struct CausalGraph {
    pub nodes: HashMap<String, GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl Default for CausalGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl CausalGraph {
    pub fn new() -> Self {
        CausalGraph {
            nodes: HashMap::new(),
            edges: Vec::new(),
        }
    }

    fn add_node(&mut self, node: GraphNode) {
        self.nodes.insert(node.id.clone(), node);
    }

    fn add_edge(&mut self, from: &str, to: &str, edge_type: EdgeType, details: &str) {
        self.edges.push(GraphEdge {
            from_id: from.to_string(),
            to_id: to.to_string(),
            edge_type,
            details: details.to_string(),
        });
    }
}

// ── Daemon query ──────────────────────────────────────────────────────────────

fn query_daemon() -> Option<serde_json::Value> {
    match crate::daemon_client::process_tree() {
        Ok(response) => Some(response),
        Err(error) => {
            eprintln!("DEGRADED cause=\"{error}\" impact=\"causal analysis may miss daemon lifecycle data\" fallback=\"one bounded procfs snapshot\" recovery=\"start `syspilot daemon` and retry\"");
            None
        }
    }
}

// ── Snapshot ──────────────────────────────────────────────────────────────────

fn take_proc_snapshot() -> HashMap<i32, GraphNode> {
    let mut snap: HashMap<i32, GraphNode> = HashMap::with_capacity(512);

    if let Some(j) = query_daemon() {
        if j["status"].as_str() == Some("ok") {
            if let Some(procs) = j["processes"].as_array() {
                for p in procs {
                    let pid = p["pid"].as_i64().unwrap_or(0) as i32;
                    if pid <= 0 {
                        continue;
                    }
                    let pt = telemetry::collect_process_telemetry(pid);
                    if pt.pid == 0 {
                        continue;
                    }
                    let node = GraphNode {
                        id: format!("pid:{}", pid),
                        node_type: NodeType::Process,
                        name: pt.name.clone(),
                        pid,
                        state: pt.state.clone(),
                        read_bytes: pt.read_bytes,
                        write_bytes: pt.write_bytes,
                        read_rate_kb: 0.0,
                        write_rate_kb: 0.0,
                        cpu_usage_pct: (pt.utime + pt.stime) as f64,
                        is_anomalous: false,
                        anomaly_reason: String::new(),
                    };
                    snap.insert(pid, node);
                }
                return snap;
            }
        }
    }

    let snapshot = crate::proc_snapshot::ProcSnapshot::shared(Duration::from_millis(250));
    for pt in &snapshot.processes {
        let pid = pt.pid;
        let node = GraphNode {
            id: format!("pid:{}", pid),
            node_type: NodeType::Process,
            name: pt.name.clone(),
            pid,
            state: pt.state.clone(),
            read_bytes: pt.read_bytes,
            write_bytes: pt.write_bytes,
            read_rate_kb: 0.0,
            write_rate_kb: 0.0,
            cpu_usage_pct: (pt.utime + pt.stime) as f64,
            is_anomalous: false,
            anomaly_reason: String::new(),
        };
        snap.insert(pid, node);
    }
    snap
}

// ── build_graph ───────────────────────────────────────────────────────────────

impl CausalGraph {
    pub fn build_graph(&mut self, interval_seconds: u64, use_ebpf: bool, target_pid: i32) {
        self.nodes.clear();
        self.edges.clear();

        let bpftrace_log = "/tmp/syspilot_bpftrace.log";
        let ebpf_running = if use_ebpf && target_pid > 0 {
            let has_priv = nix::unistd::Uid::effective().is_root()
                || utils::run_command_output("sudo -n true 2>/dev/null").1 == 0;
            if has_priv {
                if let Err(error) = fs::remove_file(bpftrace_log) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("⚠️  eBPF trace cleanup failed: {error}");
                    }
                }
                let bin = if nix::unistd::Uid::effective().is_root() {
                    "bpftrace"
                } else {
                    "sudo bpftrace"
                };
                let filter = format!("pid == {} || ppid == {}", target_pid, target_pid);
                let script = format!(
                    "tracepoint:syscalls:sys_enter_open,tracepoint:syscalls:sys_enter_openat \
                    /{f}/ {{ printf(\"OPEN | %d | %s | %s\\n\", pid, comm, str(args->filename)); }} \
                    tracepoint:syscalls:sys_enter_connect /{f}/ {{ \
                    printf(\"CONNECT | %d | %s | family: %d\\n\", pid, comm, args->uservaddr->sa_family); }} \
                    tracepoint:syscalls:sys_enter_execve /{f}/ {{ \
                    printf(\"EXEC | %d | %s | %s\\n\", pid, comm, str(args->filename)); }}",
                    f = filter
                );
                let cmd = format!(
                    "timeout {} {} -e '{}' > {} 2>/dev/null &",
                    interval_seconds, bin, script, bpftrace_log
                );
                utils::run_command_output(&cmd);
                println!("🚀 [eBPF] Active tracing enabled for PID {}...", target_pid);
                true
            } else {
                println!("⚠️  [eBPF] Insufficient privileges. Falling back to standard procfs polling...");
                false
            }
        } else {
            false
        };

        // Snapshot 1
        let snap1 = take_proc_snapshot();
        let disk1 = telemetry::get_disk_stats();

        std::thread::sleep(Duration::from_secs(interval_seconds));

        // Snapshot 2
        let snap2 = take_proc_snapshot();
        let disk2 = telemetry::get_disk_stats();

        let clk_tck = nix::unistd::sysconf(nix::unistd::SysconfVar::CLK_TCK)
            .ok()
            .flatten()
            .unwrap_or(100) as f64;
        let clk_tck = if clk_tck <= 0.0 { 100.0 } else { clk_tck };

        // Populate process nodes with rates
        for (pid, mut node) in snap2 {
            if let Some(prev) = snap1.get(&pid) {
                let cpu_delta = node.cpu_usage_pct - prev.cpu_usage_pct;
                node.cpu_usage_pct = (cpu_delta / (clk_tck * interval_seconds as f64)) * 100.0;
                node.read_rate_kb = node.read_bytes.saturating_sub(prev.read_bytes) as f64
                    / (1024.0 * interval_seconds as f64);
                node.write_rate_kb = node.write_bytes.saturating_sub(prev.write_bytes) as f64
                    / (1024.0 * interval_seconds as f64);
            } else {
                node.cpu_usage_pct = 0.0;
                node.read_rate_kb = 0.0;
                node.write_rate_kb = 0.0;
            }

            // Anomaly detection
            if node.state == "D" {
                node.is_anomalous = true;
                node.anomaly_reason =
                    "Process is in uninterruptible sleep (D-state), likely blocked on disk I/O"
                        .to_string();
            } else if node.cpu_usage_pct > 80.0 {
                node.is_anomalous = true;
                node.anomaly_reason = format!("High CPU utilization ({:.0}%)", node.cpu_usage_pct);
            } else if node.write_rate_kb > 5000.0 {
                node.is_anomalous = true;
                node.anomaly_reason =
                    format!("High disk write rate ({:.0} KB/s)", node.write_rate_kb);
            } else if node.read_rate_kb > 5000.0 {
                node.is_anomalous = true;
                node.anomaly_reason =
                    format!("High disk read rate ({:.0} KB/s)", node.read_rate_kb);
            }

            self.add_node(node);
        }

        // Device nodes from disk stats
        let mut dev_nodes: Vec<GraphNode> = Vec::new();
        for (key, &io_time2) in &disk2 {
            if !key.ends_with("_io_time_ms") {
                continue;
            }
            let dev_name = &key[..key.len() - 11];
            let io_time1 = disk1.get(key).copied().unwrap_or(0);
            let io_delta = io_time2.saturating_sub(io_time1) as f64;
            let io_util = (io_delta / (interval_seconds as f64 * 1000.0)) * 100.0;

            let r_bytes = disk2
                .get(&format!("{}_read_bytes", dev_name))
                .copied()
                .unwrap_or(0)
                .saturating_sub(
                    disk1
                        .get(&format!("{}_read_bytes", dev_name))
                        .copied()
                        .unwrap_or(0),
                );
            let w_bytes = disk2
                .get(&format!("{}_write_bytes", dev_name))
                .copied()
                .unwrap_or(0)
                .saturating_sub(
                    disk1
                        .get(&format!("{}_write_bytes", dev_name))
                        .copied()
                        .unwrap_or(0),
                );

            let dev = GraphNode {
                id: format!("resource:/dev/{}", dev_name),
                node_type: NodeType::Resource,
                name: format!("/dev/{}", dev_name),
                pid: 0,
                state: String::new(),
                read_bytes: 0,
                write_bytes: 0,
                read_rate_kb: r_bytes as f64 / (1024.0 * interval_seconds as f64),
                write_rate_kb: w_bytes as f64 / (1024.0 * interval_seconds as f64),
                cpu_usage_pct: 0.0,
                is_anomalous: io_util > 80.0,
                anomaly_reason: if io_util > 80.0 {
                    format!("Device utilization is high ({:.0}% time active)", io_util)
                } else {
                    String::new()
                },
            };
            dev_nodes.push(dev);
        }
        for node in dev_nodes {
            self.add_node(node);
        }

        // Identify active files
        let node_ids: Vec<String> = self.nodes.keys().cloned().collect();
        let mut active_files: HashSet<String> = HashSet::new();
        for id in &node_ids {
            if let Some(node) = self.nodes.get(id) {
                if node.node_type == NodeType::Process
                    && (node.read_rate_kb > 0.1 || node.write_rate_kb > 0.1)
                {
                    for (_, path) in telemetry::get_open_resources(node.pid) {
                        if path.contains("/var/lib/")
                            || path.contains("/tmp/")
                            || path.contains("/home/")
                        {
                            active_files.insert(path);
                        }
                    }
                }
            }
        }

        // Connect processes to resources
        let mut new_resource_nodes: Vec<GraphNode> = Vec::new();
        let mut new_edges: Vec<GraphEdge> = Vec::new();

        for id in node_ids.clone() {
            let (pid, is_writing, state, node_id) = {
                if let Some(node) = self.nodes.get(&id) {
                    if node.node_type != NodeType::Process {
                        continue;
                    }
                    (
                        node.pid,
                        node.write_rate_kb > 0.1,
                        node.state.clone(),
                        node.id.clone(),
                    )
                } else {
                    continue;
                }
            };

            let pt = telemetry::collect_process_telemetry(pid);

            // Parent edge
            if pt.ppid != 0 {
                let parent_id = format!("pid:{}", pt.ppid);
                if self.nodes.contains_key(&parent_id) {
                    new_edges.push(GraphEdge {
                        from_id: node_id.clone(),
                        to_id: parent_id,
                        edge_type: EdgeType::SpawnedBy,
                        details: String::new(),
                    });
                }
            }

            let open_res = telemetry::get_open_resources(pid);
            let mut mapped: HashSet<String> = HashSet::new();

            for (fd_name, path) in open_res {
                let resource_id = if path.starts_with("/dev/") {
                    Some(format!("resource:{}", path))
                } else if path.starts_with("socket:[") {
                    if is_writing {
                        Some(format!("resource:{}", path))
                    } else {
                        None
                    }
                } else if active_files.contains(&path) {
                    Some(format!("resource:{}", path))
                } else {
                    None
                };

                if let Some(rid) = resource_id {
                    if mapped.insert(rid.clone()) {
                        if !self.nodes.contains_key(&rid) {
                            new_resource_nodes.push(GraphNode {
                                id: rid.clone(),
                                node_type: NodeType::Resource,
                                name: path.clone(),
                                pid: 0,
                                state: String::new(),
                                read_bytes: 0,
                                write_bytes: 0,
                                read_rate_kb: 0.0,
                                write_rate_kb: 0.0,
                                cpu_usage_pct: 0.0,
                                is_anomalous: false,
                                anomaly_reason: String::new(),
                            });
                        }
                        let etype = if is_writing {
                            EdgeType::WritesTo
                        } else {
                            EdgeType::ReadsFrom
                        };
                        new_edges.push(GraphEdge {
                            from_id: node_id.clone(),
                            to_id: rid.clone(),
                            edge_type: etype,
                            details: format!("FD: {}", fd_name),
                        });
                        if state == "D" {
                            new_edges.push(GraphEdge {
                                from_id: node_id.clone(),
                                to_id: rid.clone(),
                                edge_type: EdgeType::BlockedOn,
                                details: "Process blocked in I/O wait on resource".to_string(),
                            });
                        }
                    }
                }
            }

            // D-state with no block edge: link to most active disk
            if state == "D" {
                let has_blocked = new_edges
                    .iter()
                    .any(|e| e.from_id == node_id && e.edge_type == EdgeType::BlockedOn);
                if !has_blocked {
                    let target_disk = self
                        .nodes
                        .iter()
                        .filter(|(_, n)| {
                            n.node_type == NodeType::Resource && n.id.starts_with("resource:/dev/")
                        })
                        .max_by(|(_, a), (_, b)| {
                            let a_io = a.read_rate_kb + a.write_rate_kb;
                            let b_io = b.read_rate_kb + b.write_rate_kb;
                            a_io.partial_cmp(&b_io).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(id, _)| id.clone())
                        .unwrap_or_else(|| "resource:/dev/sda".to_string());

                    new_edges.push(GraphEdge {
                        from_id: node_id.clone(),
                        to_id: target_disk,
                        edge_type: EdgeType::BlockedOn,
                        details: "Process blocked on primary disk device".to_string(),
                    });
                }
            }
        }

        // Add new resource nodes (deduplicated)
        for node in new_resource_nodes {
            if !self.nodes.contains_key(&node.id) {
                self.add_node(node);
            }
        }
        self.edges.extend(new_edges);

        // CONTENDS_WITH edges
        let mut resource_accessors: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for edge in &self.edges {
            if matches!(
                edge.edge_type,
                EdgeType::ReadsFrom | EdgeType::WritesTo | EdgeType::BlockedOn
            ) {
                if let Some(node) = self.nodes.get(&edge.from_id) {
                    let io = node.read_rate_kb + node.write_rate_kb;
                    resource_accessors
                        .entry(edge.to_id.clone())
                        .or_default()
                        .push((edge.from_id.clone(), io));
                }
            }
        }
        let mut contend_edges: Vec<GraphEdge> = Vec::new();
        for (res_id, accessors) in &resource_accessors {
            let active: Vec<&(String, f64)> =
                accessors.iter().filter(|(_, io)| *io > 500.0).collect();
            if active.len() > 1 {
                let details = format!("Shared access to {}", res_id);
                for i in 0..active.len() {
                    for j in (i + 1)..active.len() {
                        contend_edges.push(GraphEdge {
                            from_id: active[i].0.clone(),
                            to_id: active[j].0.clone(),
                            edge_type: EdgeType::ContendsWith,
                            details: details.clone(),
                        });
                        contend_edges.push(GraphEdge {
                            from_id: active[j].0.clone(),
                            to_id: active[i].0.clone(),
                            edge_type: EdgeType::ContendsWith,
                            details: details.clone(),
                        });
                    }
                }
            }
        }
        self.edges.extend(contend_edges);

        // Parse eBPF log
        if ebpf_running {
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(content) = fs::read_to_string(bpftrace_log) {
                for line in content.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    let parts: Vec<&str> = line.splitn(4, " | ").collect();
                    if parts.len() < 4 {
                        continue;
                    }
                    let event_type = parts[0].trim();
                    let event_pid: i32 = match parts[1].trim().parse() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let comm = parts[2].trim();
                    let details = parts[3].trim();
                    let pnode_id = format!("pid:{}", event_pid);

                    if !self.nodes.contains_key(&pnode_id) {
                        self.add_node(GraphNode {
                            id: pnode_id.clone(),
                            node_type: NodeType::Process,
                            name: comm.to_string(),
                            pid: event_pid,
                            state: "S".to_string(),
                            read_bytes: 0,
                            write_bytes: 0,
                            read_rate_kb: 0.0,
                            write_rate_kb: 0.0,
                            cpu_usage_pct: 0.0,
                            is_anomalous: false,
                            anomaly_reason: String::new(),
                        });
                    }

                    let (res_id, res_name) = match event_type {
                        "OPEN" | "OPENAT" => (format!("resource:{}", details), details.to_string()),
                        "CONNECT" => (
                            format!("resource:socket_{}", details),
                            format!("Socket ({})", details),
                        ),
                        "EXEC" => (format!("resource:{}", details), details.to_string()),
                        _ => continue,
                    };

                    if !self.nodes.contains_key(&res_id) {
                        self.add_node(GraphNode {
                            id: res_id.clone(),
                            node_type: NodeType::Resource,
                            name: res_name,
                            pid: 0,
                            state: String::new(),
                            read_bytes: 0,
                            write_bytes: 0,
                            read_rate_kb: 0.0,
                            write_rate_kb: 0.0,
                            cpu_usage_pct: 0.0,
                            is_anomalous: false,
                            anomaly_reason: String::new(),
                        });
                    }
                    self.add_edge(
                        &pnode_id,
                        &res_id,
                        EdgeType::ReadsFrom,
                        &format!("eBPF: {}", event_type.to_lowercase()),
                    );
                }
            }
            if let Err(error) = fs::remove_file(bpftrace_log) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("⚠️  eBPF trace cleanup failed: {error}");
                }
            }
        }
    }
}

// ── BFS root cause tracer ─────────────────────────────────────────────────────

impl CausalGraph {
    pub fn trace_root_cause(&self, start_node_id: &str) -> Vec<String> {
        let mut path: Vec<String> = Vec::new();
        if !self.nodes.contains_key(start_node_id) {
            return path;
        }

        let mut queue: VecDeque<String> = VecDeque::new();
        let mut visited: HashSet<String> = HashSet::new();

        queue.push_back(start_node_id.to_string());
        visited.insert(start_node_id.to_string());

        while let Some(current) = queue.pop_front() {
            path.push(current.clone());

            if let Some(node) = self.nodes.get(&current) {
                if node.node_type == NodeType::Process {
                    for edge in &self.edges {
                        if edge.from_id != current {
                            continue;
                        }
                        match edge.edge_type {
                            EdgeType::BlockedOn | EdgeType::SpawnedBy | EdgeType::ContendsWith => {
                                if visited.insert(edge.to_id.clone()) {
                                    queue.push_back(edge.to_id.clone());
                                }
                            }
                            EdgeType::ReadsFrom | EdgeType::WritesTo => {
                                if let Some(res) = self.nodes.get(&edge.to_id) {
                                    let traverse = res.is_anomalous
                                        || self.edges.iter().any(|e| {
                                            e.to_id == edge.to_id
                                                && e.from_id != current
                                                && self
                                                    .nodes
                                                    .get(&e.from_id)
                                                    .map(|n| n.is_anomalous)
                                                    .unwrap_or(false)
                                        });
                                    if traverse && visited.insert(edge.to_id.clone()) {
                                        queue.push_back(edge.to_id.clone());
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Resource node: find top-3 processes by IO accessing it
                    let mut writers: Vec<(String, f64)> = self
                        .edges
                        .iter()
                        .filter(|e| {
                            e.to_id == current
                                && matches!(e.edge_type, EdgeType::WritesTo | EdgeType::ReadsFrom)
                        })
                        .filter_map(|e| {
                            self.nodes
                                .get(&e.from_id)
                                .map(|n| (e.from_id.clone(), n.write_rate_kb + n.read_rate_kb))
                        })
                        .collect();
                    writers
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    for (pid_id, _) in writers.iter().take(3) {
                        if visited.insert(pid_id.clone()) {
                            queue.push_back(pid_id.clone());
                        }
                    }
                }
            }
        }
        path
    }

    pub fn serialize_chain_to_json(&self, path_nodes: &[String]) -> String {
        let path_set: HashSet<&str> = path_nodes.iter().map(|s| s.as_str()).collect();

        let nodes_json: Vec<serde_json::Value> = path_nodes
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .map(|n| {
                let mut obj = serde_json::json!({
                    "id": n.id,
                    "type": if n.node_type == NodeType::Process { "process" } else { "resource" },
                    "name": n.name,
                    "read_rate_kb": n.read_rate_kb,
                    "write_rate_kb": n.write_rate_kb,
                    "is_anomalous": n.is_anomalous,
                    "anomaly_reason": n.anomaly_reason,
                });
                if n.node_type == NodeType::Process {
                    obj["pid"] = serde_json::json!(n.pid);
                    obj["state"] = serde_json::json!(n.state);
                    obj["cpu_usage_pct"] = serde_json::json!(n.cpu_usage_pct);
                }
                obj
            })
            .collect();

        let edges_json: Vec<serde_json::Value> = self
            .edges
            .iter()
            .filter(|e| {
                path_set.contains(e.from_id.as_str()) && path_set.contains(e.to_id.as_str())
            })
            .map(|e| {
                serde_json::json!({
                    "from": e.from_id,
                    "to": e.to_id,
                    "type": e.edge_type.as_str(),
                    "details": e.details,
                })
            })
            .collect();

        serde_json::json!({
            "symptom_node": path_nodes.first().map(|s| s.as_str()).unwrap_or(""),
            "nodes": nodes_json,
            "edges": edges_json,
        })
        .to_string()
    }

    pub fn export_graph_to_dot(&self, path_nodes: &[String]) -> String {
        let path_set: HashSet<&str> = path_nodes.iter().map(|s| s.as_str()).collect();
        let mut out = String::from("digraph CausalTrace {\n");
        out.push_str("    node [style=\"filled,rounded\", shape=box];\n");
        out.push_str("    edge [color=\"#94a3b8\"];\n\n");

        for id in path_nodes {
            if let Some(n) = self.nodes.get(id) {
                let label = if n.node_type == NodeType::Process {
                    format!("{} (PID: {})", n.name, n.pid)
                } else {
                    format!("[Resource]\\n{}", n.name)
                };
                let fill = if n.is_anomalous {
                    "#7f1d1d"
                } else if n.node_type == NodeType::Resource {
                    "#0f172a"
                } else {
                    "#0284c7"
                };
                let stroke = if n.is_anomalous {
                    "#ef4444"
                } else if n.node_type == NodeType::Resource {
                    "#334155"
                } else {
                    "#38bdf8"
                };
                out.push_str(&format!(
                    "    \"{}\" [label=\"{}\", fillcolor=\"{}\", color=\"{}\", fontcolor=\"#f8fafc\"];\n",
                    id, label, fill, stroke
                ));
            }
        }
        out.push('\n');

        for e in &self.edges {
            if path_set.contains(e.from_id.as_str()) && path_set.contains(e.to_id.as_str()) {
                let color = match e.edge_type {
                    EdgeType::WritesTo => "#f97316",
                    EdgeType::ReadsFrom => "#38bdf8",
                    EdgeType::BlockedOn => "#ef4444",
                    EdgeType::ContendsWith => "#eab308",
                    EdgeType::SpawnedBy => "#475569",
                };
                out.push_str(&format!(
                    "    \"{}\" -> \"{}\" [label=\"{}\", color=\"{}\"];\n",
                    e.from_id,
                    e.to_id,
                    e.edge_type.as_str(),
                    color
                ));
            }
        }
        out.push_str("}\n");
        out
    }

    pub fn export_graph_to_html(&self, path_nodes: &[String]) -> String {
        let path_set: HashSet<&str> = path_nodes.iter().map(|s| s.as_str()).collect();

        let mut elements: Vec<serde_json::Value> = Vec::new();

        for id in path_nodes {
            if let Some(n) = self.nodes.get(id) {
                let mut data = serde_json::json!({
                    "id": n.id,
                    "name": n.name,
                    "type": if n.node_type == NodeType::Process { "process" } else { "resource" },
                    "is_anomalous": n.is_anomalous,
                    "anomaly_reason": n.anomaly_reason,
                    "read_rate_kb": n.read_rate_kb,
                    "write_rate_kb": n.write_rate_kb,
                });
                if n.node_type == NodeType::Process {
                    data["pid"] = serde_json::json!(n.pid);
                    data["state"] = serde_json::json!(n.state);
                    data["cpu_usage_pct"] = serde_json::json!(n.cpu_usage_pct);
                }
                elements.push(serde_json::json!({ "data": data }));
            }
        }

        for (i, e) in self.edges.iter().enumerate() {
            if path_set.contains(e.from_id.as_str()) && path_set.contains(e.to_id.as_str()) {
                elements.push(serde_json::json!({
                    "data": {
                        "id": format!("edge_{}", i),
                        "source": e.from_id,
                        "target": e.to_id,
                        "type": e.edge_type.as_str(),
                        "details": e.details,
                    }
                }));
            }
        }

        let elements_json = script_safe_json(
            &serde_json::to_string(&elements).unwrap_or_else(|_| "[]".to_string()),
        );
        let symptom = path_nodes
            .first()
            .and_then(|id| self.nodes.get(id))
            .map(|n| {
                if n.node_type == NodeType::Process {
                    format!("{} (PID {})", n.name, n.pid)
                } else {
                    n.name.clone()
                }
            })
            .unwrap_or_else(|| "Unknown".to_string());

        format!(
            include_str!("causal_template.html"),
            elements_json = elements_json,
            symptom_json = script_safe_json(
                &serde_json::to_string(&symptom).unwrap_or_else(|_| "\"Unknown\"".into())
            ),
        )
    }
}

fn script_safe_json(value: &str) -> String {
    value
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}
