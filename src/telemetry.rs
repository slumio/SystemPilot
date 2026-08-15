use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::Path;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProcessTelemetry {
    pub pid: i32,
    pub name: String,
    pub ppid: i32,
    pub state: String,
    pub thread_count: i32,
    pub utime: u64,
    pub stime: u64,
    pub rss_bytes: u64,
    pub vsize_bytes: u64,
    pub minflt: u64,
    pub majflt: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_char: u64,
    pub write_char: u64,
    pub syscr: u64,
    pub syscw: u64,
    pub voluntary_ctxt_switches: u64,
    pub involuntary_ctxt_switches: u64,
    pub sched_run_ticks: u64,
    pub sched_wait_ticks: u64,
    pub sched_timeslices: u64,
    pub child_pids: Vec<i32>,
    pub env: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SystemTelemetry {
    pub load_avg: String,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    pub mem_available_kb: u64,
    pub mem_cached_kb: u64,
    pub mem_buffers_kb: u64,
    pub cpu_user: u64,
    pub cpu_nice: u64,
    pub cpu_system: u64,
    pub cpu_idle: u64,
    pub cpu_iowait: u64,
    pub cpu_irq: u64,
    pub cpu_softirq: u64,
    pub disk_usage_summary: String,
}

fn parse_u64(s: &str) -> u64 {
    s.trim().parse().unwrap_or(0)
}

fn parse_i32(s: &str) -> i32 {
    s.trim().parse().unwrap_or(0)
}

/// Find the PID for a process by name or numeric PID string.
pub fn find_pid_by_name(name: &str) -> i32 {
    if name.is_empty() {
        return 0;
    }
    // Check if it's a numeric PID
    if name.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(pid) = name.parse::<i32>() {
            if pid > 0 && Path::new(&format!("/proc/{}", pid)).exists() {
                return pid;
            }
        }
    }

    let proc_dir = match fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return 0,
    };

    for entry in proc_dir.flatten() {
        let fname = entry.file_name();
        let pid_str = fname.to_string_lossy();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let pid: i32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Check /proc/<pid>/comm
        let comm_path = format!("/proc/{}/comm", pid);
        if let Ok(comm) = fs::read_to_string(&comm_path) {
            if comm.trim() == name {
                return pid;
            }
        }

        // Fallback: /proc/<pid>/cmdline
        let cmd_path = format!("/proc/{}/cmdline", pid);
        if let Ok(raw) = fs::read(&cmd_path) {
            if let Some(nul) = raw.iter().position(|&b| b == 0) {
                let binary = String::from_utf8_lossy(&raw[..nul]);
                let base = binary.rsplit('/').next().unwrap_or(&binary);
                if base == name {
                    return pid;
                }
            }
        }
    }
    0
}

/// Collect full process telemetry, including the comparatively expensive child
/// process scan used by detailed diagnostics.
pub fn collect_process_telemetry(pid: i32) -> ProcessTelemetry {
    collect_process_telemetry_impl(pid, true)
}

/// Collect the fields required for the live monitor without re-scanning `/proc`
/// for each process to build child lists.
pub fn collect_process_telemetry_basic(pid: i32) -> ProcessTelemetry {
    collect_process_telemetry_impl(pid, false)
}

fn collect_process_telemetry_impl(pid: i32, include_children: bool) -> ProcessTelemetry {
    let mut pt = ProcessTelemetry {
        pid,
        ..Default::default()
    };
    let pid_dir = format!("/proc/{}", pid);
    if !Path::new(&pid_dir).exists() {
        return pt;
    }

    // 1. /proc/<pid>/comm
    if let Ok(s) = fs::read_to_string(format!("{}/comm", pid_dir)) {
        pt.name = s.trim().to_string();
    }

    // 2. /proc/<pid>/stat
    if let Ok(stat) = fs::read_to_string(format!("{}/stat", pid_dir)) {
        // Format: pid (name) state ppid ...
        if let Some(close_paren) = stat.rfind(')') {
            let rest = &stat[close_paren + 2..];
            let fields: Vec<&str> = rest.split_whitespace().collect();
            // fields[0] = state (field 3), fields[1] = ppid (field 4) ...
            // Original index = field_index_after_close_paren + 3
            if !fields.is_empty() {
                pt.state = fields[0].to_string();
            }
            if fields.len() > 1 {
                pt.ppid = parse_i32(fields[1]);
            }
            if fields.len() > 7 {
                pt.minflt = parse_u64(fields[7]);
            }
            if fields.len() > 9 {
                pt.majflt = parse_u64(fields[9]);
            }
            if fields.len() > 11 {
                pt.utime = parse_u64(fields[11]);
            }
            if fields.len() > 12 {
                pt.stime = parse_u64(fields[12]);
            }
            if fields.len() > 17 {
                pt.thread_count = parse_i32(fields[17]);
            }
            if fields.len() > 20 {
                pt.vsize_bytes = parse_u64(fields[20]);
            }
            if fields.len() > 21 {
                let page_size = nix::unistd::sysconf(nix::unistd::SysconfVar::PAGE_SIZE)
                    .ok()
                    .flatten()
                    .unwrap_or(4096) as u64;
                let page_size = if page_size == 0 { 4096 } else { page_size };
                pt.rss_bytes = parse_u64(fields[21]).saturating_mul(page_size);
            }
        }
    }

    // 3. /proc/<pid>/status — voluntary/involuntary context switches
    if let Ok(f) = fs::File::open(format!("{}/status", pid_dir)) {
        for line in io::BufReader::new(f).lines().map_while(Result::ok) {
            if let Some(val) = line.strip_prefix("voluntary_ctxt_switches:") {
                pt.voluntary_ctxt_switches = parse_u64(val);
            } else if let Some(val) = line.strip_prefix("nonvoluntary_ctxt_switches:") {
                pt.involuntary_ctxt_switches = parse_u64(val);
            }
        }
    }

    // 4. /proc/<pid>/schedstat
    if let Ok(s) = fs::read_to_string(format!("{}/schedstat", pid_dir)) {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() >= 3 {
            pt.sched_run_ticks = parse_u64(parts[0]);
            pt.sched_wait_ticks = parse_u64(parts[1]);
            pt.sched_timeslices = parse_u64(parts[2]);
        }
    }

    // 5. /proc/<pid>/io
    if let Ok(f) = fs::File::open(format!("{}/io", pid_dir)) {
        for line in io::BufReader::new(f).lines().map_while(Result::ok) {
            let mut parts = line.splitn(2, ':');
            if let (Some(key), Some(val)) = (parts.next(), parts.next()) {
                let v = parse_u64(val);
                match key.trim() {
                    "rchar" => pt.read_char = v,
                    "wchar" => pt.write_char = v,
                    "syscr" => pt.syscr = v,
                    "syscw" => pt.syscw = v,
                    "read_bytes" => pt.read_bytes = v,
                    "write_bytes" => pt.write_bytes = v,
                    _ => {}
                }
            }
        }
    }

    // 6. /proc/<pid>/environ
    if let Ok(raw) = fs::read(format!("{}/environ", pid_dir)) {
        let mut vars: Vec<String> = raw
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        vars.truncate(30); // Don't blow AI context
        pt.env = vars;
    }

    // 7. Child PIDs. This is intentionally skipped by the live monitor: doing
    // it for every displayed process would turn a single refresh into O(n²)
    // scans of /proc.
    if include_children {
        if let Ok(proc_dir) = fs::read_dir("/proc") {
            for entry in proc_dir.flatten() {
                let fname = entry.file_name();
                let pid_str = fname.to_string_lossy();
                if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let child_pid: i32 = match pid_str.parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let stat_path = format!("/proc/{}/stat", child_pid);
                if let Ok(stat) = fs::read_to_string(&stat_path) {
                    if let Some(cp) = stat.rfind(')') {
                        let rest = &stat[cp + 2..];
                        let fields: Vec<&str> = rest.split_whitespace().collect();
                        if fields.len() > 1 && parse_i32(fields[1]) == pid {
                            pt.child_pids.push(child_pid);
                        }
                    }
                }
            }
        }
    }

    pt
}

pub fn collect_system_telemetry() -> SystemTelemetry {
    let mut st = SystemTelemetry::default();

    // 1. /proc/loadavg
    if let Ok(s) = fs::read_to_string("/proc/loadavg") {
        st.load_avg = s.trim().to_string();
    }

    // 2. /proc/meminfo
    if let Ok(f) = fs::File::open("/proc/meminfo") {
        for line in io::BufReader::new(f).lines().map_while(Result::ok) {
            let mut parts = line.splitn(2, ':');
            if let (Some(key), Some(val)) = (parts.next(), parts.next()) {
                // Strip " kB"
                let val_stripped = val.trim().trim_end_matches(" kB").trim();
                let v = parse_u64(val_stripped);
                match key.trim() {
                    "MemTotal" => st.mem_total_kb = v,
                    "MemFree" => st.mem_free_kb = v,
                    "MemAvailable" => st.mem_available_kb = v,
                    "Cached" => st.mem_cached_kb = v,
                    "Buffers" => st.mem_buffers_kb = v,
                    _ => {}
                }
            }
        }
    }

    // 3. /proc/stat — first line is aggregate CPU
    if let Ok(f) = fs::File::open("/proc/stat") {
        let mut lines = io::BufReader::new(f).lines();
        if let Some(Ok(line)) = lines.next() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 8 && fields[0] == "cpu" {
                st.cpu_user = parse_u64(fields[1]);
                st.cpu_nice = parse_u64(fields[2]);
                st.cpu_system = parse_u64(fields[3]);
                st.cpu_idle = parse_u64(fields[4]);
                st.cpu_iowait = parse_u64(fields[5]);
                st.cpu_irq = parse_u64(fields[6]);
                st.cpu_softirq = parse_u64(fields[7]);
            }
        }
    }

    // 4. Disk usage summary
    let (out, _) = crate::utils::run_command_output("df -h / 2>/dev/null");
    st.disk_usage_summary = out.trim().to_string();

    st
}

pub fn serialize_telemetry_to_json(pt: &ProcessTelemetry, st: &SystemTelemetry) -> String {
    let mut env = pt.env.clone();
    if pt.env.len() > 30 {
        env.truncate(30);
        env.push(format!(
            "... (truncated {} environment variables)",
            pt.env.len() - 30
        ));
    }

    let process = serde_json::json!({
        "pid": pt.pid,
        "name": pt.name,
        "ppid": pt.ppid,
        "state": pt.state,
        "thread_count": pt.thread_count,
        "cpu_user_ticks": pt.utime,
        "cpu_system_ticks": pt.stime,
        "rss_bytes": pt.rss_bytes,
        "vsize_bytes": pt.vsize_bytes,
        "minor_page_faults": pt.minflt,
        "major_page_faults": pt.majflt,
        "voluntary_context_switches": pt.voluntary_ctxt_switches,
        "involuntary_context_switches": pt.involuntary_ctxt_switches,
        "io_read_bytes": pt.read_bytes,
        "io_write_bytes": pt.write_bytes,
        "io_read_chars": pt.read_char,
        "io_write_chars": pt.write_char,
        "io_syscr": pt.syscr,
        "io_syscw": pt.syscw,
        "sched_run_ticks": pt.sched_run_ticks,
        "sched_wait_ticks": pt.sched_wait_ticks,
        "sched_timeslices": pt.sched_timeslices,
        "child_pids": pt.child_pids,
        "environment": env,
    });

    let system = serde_json::json!({
        "load_average": st.load_avg,
        "memory_total_kb": st.mem_total_kb,
        "memory_free_kb": st.mem_free_kb,
        "memory_available_kb": st.mem_available_kb,
        "memory_cached_kb": st.mem_cached_kb,
        "memory_buffers_kb": st.mem_buffers_kb,
        "cpu_ticks": {
            "user": st.cpu_user,
            "nice": st.cpu_nice,
            "system": st.cpu_system,
            "idle": st.cpu_idle,
            "iowait": st.cpu_iowait,
            "irq": st.cpu_irq,
            "softirq": st.cpu_softirq,
        },
        "disk_usage": st.disk_usage_summary,
    });

    serde_json::json!({ "process": process, "system": system }).to_string()
}

pub fn collect_ebpf_telemetry(pid: i32, duration_seconds: u32) -> String {
    let has_privileges = nix::unistd::Uid::effective().is_root()
        || crate::utils::run_command_output("sudo -n true 2>/dev/null").1 == 0;

    if !has_privileges {
        eprintln!(
            "\n⚠️  eBPF tracing requires root privileges. Please run SysPilot with sudo:\n   sudo ./syspilot explain --pid {} --ebpf",
            pid
        );
        return "Error: Insufficient privileges to run eBPF bpftrace.".to_string();
    }

    println!(
        "🚀 Starting real-time eBPF event tracing on PID {} ({}s)...",
        pid, duration_seconds
    );

    let pt = collect_process_telemetry(pid);
    let mut filter = format!("pid == {} || ppid == {}", pid, pid);
    for child in &pt.child_pids {
        filter.push_str(&format!(" || pid == {}", child));
    }

    let script = format!(
        "tracepoint:syscalls:sys_enter_open,tracepoint:syscalls:sys_enter_openat /{filter}/ {{ \
          printf(\"OPEN | %s | %s\\n\", comm, str(args->filename)); \
        }} \
        tracepoint:syscalls:sys_enter_connect /{filter}/ {{ \
          printf(\"CONNECT | %s | family: %d\\n\", comm, args->uservaddr->sa_family); \
        }} \
        tracepoint:syscalls:sys_enter_execve /{filter}/ {{ \
          printf(\"EXEC | %s | %s\\n\", comm, str(args->filename)); \
        }}",
        filter = filter
    );

    let bpftrace_bin = if nix::unistd::Uid::effective().is_root() {
        "bpftrace"
    } else {
        "sudo bpftrace"
    };

    let cmd = format!(
        "timeout {} {} -e '{}' 2>/dev/null",
        duration_seconds, bpftrace_bin, script
    );
    let (output, _) = crate::utils::run_command_output(&cmd);

    let mut opens = Vec::new();
    let mut connects = Vec::new();
    let mut execs = Vec::new();

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("OPEN | ") {
            opens.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("CONNECT | ") {
            connects.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("EXEC | ") {
            execs.push(rest.to_string());
        }
    }

    let mut summary = format!(
        "=== eBPF Kernel Event Log (Duration: {}s) ===\n",
        duration_seconds
    );
    if !opens.is_empty() {
        summary.push_str("📂 File Operations (Opened):\n");
        for f in &opens {
            summary.push_str(&format!("  - {}\n", f));
        }
    }
    if !connects.is_empty() {
        summary.push_str("🌐 Network Operations:\n");
        for c in &connects {
            summary.push_str(&format!("  - {}\n", c));
        }
    }
    if !execs.is_empty() {
        summary.push_str("🚀 Process Executions:\n");
        for e in &execs {
            summary.push_str(&format!("  - {}\n", e));
        }
    }
    if opens.is_empty() && connects.is_empty() && execs.is_empty() {
        summary.push_str("No traced events detected during the profiling window.\n");
    }
    summary
}

pub fn get_open_resources(pid: i32) -> Vec<(String, String)> {
    let fd_dir = format!("/proc/{}/fd", pid);
    let mut resources = Vec::new();
    if let Ok(dir) = fs::read_dir(&fd_dir) {
        for entry in dir.flatten() {
            let fd_name = entry.file_name().to_string_lossy().into_owned();
            if let Ok(target) = fs::read_link(entry.path()) {
                resources.push((fd_name, target.to_string_lossy().into_owned()));
            }
        }
    }
    resources
}

pub fn get_disk_stats() -> HashMap<String, u64> {
    let mut stats = HashMap::new();
    if let Ok(content) = fs::read_to_string("/proc/diskstats") {
        for line in content.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 14 {
                let dev = fields[2].to_string();
                let reads = parse_u64(fields[3]);
                let sectors_read = parse_u64(fields[5]);
                let writes = parse_u64(fields[7]);
                let sectors_written = parse_u64(fields[9]);
                let io_time = parse_u64(fields[12]);
                stats.insert(format!("{}_reads", dev), reads);
                stats.insert(format!("{}_read_bytes", dev), sectors_read * 512);
                stats.insert(format!("{}_writes", dev), writes);
                stats.insert(format!("{}_write_bytes", dev), sectors_written * 512);
                stats.insert(format!("{}_io_time_ms", dev), io_time);
            }
        }
    }
    stats
}
