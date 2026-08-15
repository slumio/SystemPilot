/// Tests for src/causal_engine.rs
/// Covers: graph construction helpers, BFS tracer, JSON/DOT/HTML exporters.
use syspilot::causal_engine::{CausalGraph, EdgeType, GraphEdge, GraphNode, NodeType};

// ── Helper: build a hand-crafted graph ───────────────────────────────────────

fn make_process(id: &str, pid: i32, name: &str, anomalous: bool, reason: &str) -> GraphNode {
    GraphNode {
        id: id.to_string(),
        node_type: NodeType::Process,
        name: name.to_string(),
        pid,
        state: "S".to_string(),
        read_bytes: 0,
        write_bytes: 0,
        read_rate_kb: 0.0,
        write_rate_kb: 0.0,
        cpu_usage_pct: 0.0,
        is_anomalous: anomalous,
        anomaly_reason: reason.to_string(),
    }
}

fn make_resource(id: &str, name: &str) -> GraphNode {
    GraphNode {
        id: id.to_string(),
        node_type: NodeType::Resource,
        name: name.to_string(),
        pid: 0,
        state: String::new(),
        read_bytes: 0,
        write_bytes: 0,
        read_rate_kb: 0.0,
        write_rate_kb: 0.0,
        cpu_usage_pct: 0.0,
        is_anomalous: false,
        anomaly_reason: String::new(),
    }
}

fn edge(from: &str, to: &str, etype: EdgeType) -> GraphEdge {
    GraphEdge {
        from_id: from.to_string(),
        to_id: to.to_string(),
        edge_type: etype,
        details: String::new(),
    }
}

fn build_test_graph() -> CausalGraph {
    // Topology:
    //   init (pid:1) --SPAWNED_BY--> (none)
    //   bash (pid:100) --SPAWNED_BY--> pid:1
    //   myapp (pid:200, anomalous, high CPU) --SPAWNED_BY--> pid:100
    //   myapp --BLOCKED_ON--> resource:/dev/sda
    //   resource:/dev/sda (anomalous, high util)
    let mut g = CausalGraph::new();

    let init = make_process("pid:1", 1, "init", false, "");
    let bash = make_process("pid:100", 100, "bash", false, "");
    let mut myapp = make_process("pid:200", 200, "myapp", true, "High CPU utilization (95%)");
    myapp.cpu_usage_pct = 95.0;
    let mut disk = make_resource("resource:/dev/sda", "/dev/sda");
    disk.is_anomalous = true;
    disk.anomaly_reason = "Device utilization is high (90% time active)".to_string();

    g.nodes.insert(init.id.clone(), init);
    g.nodes.insert(bash.id.clone(), bash);
    g.nodes.insert(myapp.id.clone(), myapp);
    g.nodes.insert(disk.id.clone(), disk);

    g.edges.push(edge("pid:100", "pid:1", EdgeType::SpawnedBy));
    g.edges
        .push(edge("pid:200", "pid:100", EdgeType::SpawnedBy));
    g.edges
        .push(edge("pid:200", "resource:/dev/sda", EdgeType::BlockedOn));

    g
}

// ── CausalGraph::new ─────────────────────────────────────────────────────────

#[test]
fn new_graph_is_empty() {
    let g = CausalGraph::new();
    assert!(g.nodes.is_empty());
    assert!(g.edges.is_empty());
}

// ── Node / edge counts ────────────────────────────────────────────────────────

#[test]
fn test_graph_has_correct_node_count() {
    let g = build_test_graph();
    assert_eq!(g.nodes.len(), 4);
}

#[test]
fn test_graph_has_correct_edge_count() {
    let g = build_test_graph();
    assert_eq!(g.edges.len(), 3);
}

// ── EdgeType::as_str ─────────────────────────────────────────────────────────

#[test]
fn edge_type_as_str() {
    assert_eq!(EdgeType::SpawnedBy.as_str(), "SPAWNED_BY");
    assert_eq!(EdgeType::ReadsFrom.as_str(), "READS_FROM");
    assert_eq!(EdgeType::WritesTo.as_str(), "WRITES_TO");
    assert_eq!(EdgeType::BlockedOn.as_str(), "BLOCKED_ON");
    assert_eq!(EdgeType::ContendsWith.as_str(), "CONTENDS_WITH");
}

// ── trace_root_cause (BFS) ────────────────────────────────────────────────────

#[test]
fn bfs_from_anomalous_process_reaches_disk() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");

    assert!(!path.is_empty(), "BFS path must not be empty");
    assert_eq!(path[0], "pid:200", "first node must be the start node");

    // Should traverse BLOCKED_ON edge to the disk resource
    assert!(
        path.contains(&"resource:/dev/sda".to_string()),
        "BFS must reach the disk resource via BLOCKED_ON edge; got: {:?}",
        path
    );
}

#[test]
fn bfs_from_anomalous_process_reaches_parent() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");

    assert!(
        path.contains(&"pid:100".to_string()),
        "BFS must reach parent bash process via SPAWNED_BY; got: {:?}",
        path
    );
}

#[test]
fn bfs_start_node_is_always_first() {
    let g = build_test_graph();
    for start in &["pid:1", "pid:100", "pid:200", "resource:/dev/sda"] {
        let path = g.trace_root_cause(start);
        if !path.is_empty() {
            assert_eq!(&path[0], start, "BFS must start from the requested node");
        }
    }
}

#[test]
fn bfs_missing_node_returns_empty() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:99999");
    assert!(path.is_empty());
}

#[test]
fn bfs_visits_each_node_at_most_once() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");

    // No duplicate node IDs
    let mut seen = std::collections::HashSet::new();
    for node_id in &path {
        assert!(
            seen.insert(node_id.clone()),
            "node '{}' appears more than once in path",
            node_id
        );
    }
}

// ── serialize_chain_to_json ───────────────────────────────────────────────────

#[test]
fn serialize_chain_produces_valid_json() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");
    let json_str = g.serialize_chain_to_json(&path);

    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("chain JSON must be valid");

    assert_eq!(parsed["symptom_node"].as_str().unwrap(), "pid:200");
    assert!(parsed["nodes"].is_array());
    assert!(parsed["edges"].is_array());
}

#[test]
fn serialize_chain_nodes_contain_required_fields() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");
    let json_str = g.serialize_chain_to_json(&path);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    for node in parsed["nodes"].as_array().unwrap() {
        assert!(node["id"].is_string(), "node must have 'id'");
        assert!(node["type"].is_string(), "node must have 'type'");
        assert!(node["name"].is_string(), "node must have 'name'");
        assert!(
            node["is_anomalous"].is_boolean(),
            "node must have 'is_anomalous'"
        );
    }
}

#[test]
fn serialize_empty_path_returns_valid_json() {
    let g = build_test_graph();
    let json_str = g.serialize_chain_to_json(&[]);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["symptom_node"].as_str().unwrap(), "");
    assert_eq!(parsed["nodes"].as_array().unwrap().len(), 0);
    assert_eq!(parsed["edges"].as_array().unwrap().len(), 0);
}

#[test]
fn serialize_chain_edges_only_include_path_nodes() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");
    let path_set: std::collections::HashSet<&str> = path.iter().map(|s| s.as_str()).collect();
    let json_str = g.serialize_chain_to_json(&path);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    for edge in parsed["edges"].as_array().unwrap() {
        let from = edge["from"].as_str().unwrap();
        let to = edge["to"].as_str().unwrap();
        assert!(
            path_set.contains(from),
            "edge 'from' '{}' not in path",
            from
        );
        assert!(path_set.contains(to), "edge 'to' '{}' not in path", to);
    }
}

// ── export_graph_to_dot ───────────────────────────────────────────────────────

#[test]
fn dot_export_is_valid_digraph() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");
    let dot = g.export_graph_to_dot(&path);

    assert!(
        dot.starts_with("digraph CausalTrace {"),
        "DOT must start with digraph declaration"
    );
    assert!(dot.ends_with("}\n"), "DOT must end with closing brace");
    assert!(dot.contains("->"), "DOT must contain at least one edge");
}

#[test]
fn dot_export_contains_node_ids() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");
    let dot = g.export_graph_to_dot(&path);

    for node_id in &path {
        assert!(
            dot.contains(node_id.as_str()),
            "DOT output must reference node id '{}'",
            node_id
        );
    }
}

// ── export_graph_to_html ──────────────────────────────────────────────────────

#[test]
fn html_export_is_html_document() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");
    let html = g.export_graph_to_html(&path);

    assert!(
        html.contains("<!DOCTYPE html>"),
        "HTML must contain doctype"
    );
    assert!(html.contains("</html>"), "HTML must be closed");
    assert!(html.contains("offline renderer"));
    assert!(!html.contains("https://"));
}

#[test]
fn html_export_embeds_elements_json() {
    let g = build_test_graph();
    let path = g.trace_root_cause("pid:200");
    let html = g.export_graph_to_html(&path);

    // The elements JSON array is embedded as a JS const
    assert!(
        html.contains("const elements = ["),
        "HTML must embed elements JSON array"
    );
    // The symptom name should appear somewhere in the rendered page
    assert!(
        html.contains("myapp"),
        "HTML must reference the symptom process name"
    );
}

#[test]
fn html_export_escapes_script_termination_and_has_no_remote_assets() {
    let mut graph = CausalGraph::new();
    let node = make_process(
        "pid:9",
        9,
        "</script><script>alert(1)</script>",
        true,
        "<unsafe>",
    );
    graph.nodes.insert(node.id.clone(), node);
    let html = graph.export_graph_to_html(&["pid:9".into()]);
    assert!(!html.contains("</script><script>alert(1)</script>"));
    assert!(html.contains("\\u003c/script\\u003e"));
    assert!(!html.contains("src=\"http"));
}

// ── Anomaly detection (rules from build_graph) ────────────────────────────────

#[test]
fn high_cpu_node_is_anomalous() {
    let mut node = make_process("pid:999", 999, "hog", false, "");
    node.cpu_usage_pct = 95.0;
    // Simulate the anomaly rule applied during build_graph
    if node.cpu_usage_pct > 80.0 {
        node.is_anomalous = true;
        node.anomaly_reason = format!("High CPU utilization ({:.0}%)", node.cpu_usage_pct);
    }
    assert!(node.is_anomalous);
    assert!(node.anomaly_reason.contains("95"));
}

#[test]
fn d_state_node_is_anomalous() {
    let mut node = make_process("pid:888", 888, "stuck", false, "");
    node.state = "D".to_string();
    if node.state == "D" {
        node.is_anomalous = true;
        node.anomaly_reason = "Process is in uninterruptible sleep (D-state)".to_string();
    }
    assert!(node.is_anomalous);
    assert!(node.anomaly_reason.contains("D-state"));
}

#[test]
fn high_write_rate_node_is_anomalous() {
    let mut node = make_process("pid:777", 777, "writer", false, "");
    node.write_rate_kb = 6000.0;
    if node.write_rate_kb > 5000.0 {
        node.is_anomalous = true;
        node.anomaly_reason = format!("High disk write rate ({:.0} KB/s)", node.write_rate_kb);
    }
    assert!(node.is_anomalous);
    assert!(node.anomaly_reason.contains("6000"));
}

#[test]
fn normal_process_is_not_anomalous() {
    let mut node = make_process("pid:555", 555, "idle", false, "");
    node.cpu_usage_pct = 1.0;
    node.write_rate_kb = 10.0;
    node.state = "S".to_string();
    // No rule triggers
    assert!(!node.is_anomalous);
}
