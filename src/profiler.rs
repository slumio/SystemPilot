use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StackTrace {
    pub tid: i32,
    pub frames: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProfileReport {
    pub perf_available: bool,
    pub top_symbols: Vec<(String, f64)>, // (symbol_name, overhead_pct)
    pub call_graph_summary: String,
    pub active_stacks: Vec<StackTrace>,
}

fn read_thread_stack(pid: i32, tid: i32) -> Vec<String> {
    let stack_path = format!("/proc/{}/task/{}/stack", pid, tid);
    let fallback = format!("/proc/{}/stack", pid);
    let path = if Path::new(&stack_path).exists() {
        stack_path
    } else if pid == tid {
        fallback
    } else {
        eprintln!(
            "⚠️  [profiler] Cannot read kernel stack for TID {} (requires CAP_SYS_ADMIN / run with sudo)",
            tid
        );
        return Vec::new();
    };

    match fs::read_to_string(&path) {
        Ok(content) => content
            .lines()
            .filter_map(|line| {
                // Parse: "[<addr>] symbol+offset/size" or "[<0>] ..."
                if let Some(close_bracket) = line.find(']') {
                    let frame = line[close_bracket + 1..].trim().to_string();
                    if !frame.is_empty() && frame != "0xffffffffffffffff" {
                        return Some(frame);
                    }
                }
                None
            })
            .collect(),
        Err(_) => {
            eprintln!(
                "⚠️  [profiler] Cannot read kernel stack for TID {} (requires CAP_SYS_ADMIN / run with sudo)",
                tid
            );
            Vec::new()
        }
    }
}

pub fn profile_process(pid: i32, run_perf: bool) -> ProfileReport {
    let mut report = ProfileReport::default();
    let pid_str = pid.to_string();
    if !Path::new(&format!("/proc/{}", pid_str)).exists() {
        return report;
    }

    // 1. Gather thread stack traces
    let task_dir = format!("/proc/{}/task", pid);
    if Path::new(&task_dir).exists() {
        if let Ok(entries) = fs::read_dir(&task_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let tid_str = fname.to_string_lossy();
                if !tid_str.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let tid: i32 = match tid_str.parse() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let frames = read_thread_stack(pid, tid);
                if !frames.is_empty() {
                    report.active_stacks.push(StackTrace { tid, frames });
                }
            }
        }
    } else {
        let frames = read_thread_stack(pid, pid);
        if !frames.is_empty() {
            report.active_stacks.push(StackTrace { tid: pid, frames });
        }
    }

    // 2. perf profiling if requested
    if run_perf {
        let (_, which_code) = crate::utils::run_command_output("which perf");
        if which_code == 0 {
            report.perf_available = true;

            let perf_data = format!("/tmp/syspilot_perf_{}.data", pid);
            let record_cmd = format!(
                "perf record -F 99 -g -o {} -p {} -- sleep 1.5",
                perf_data, pid
            );
            crate::utils::run_command_output(&record_cmd);

            let report_cmd = format!(
                "perf report -i {} --stdio --no-children --max-stack 12",
                perf_data
            );
            let (perf_out, _) = crate::utils::run_command_output(&report_cmd);

            // Clean up
            for path in [&perf_data, "perf.data"] {
                if let Err(error) = fs::remove_file(path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("⚠️  Profiler cleanup failed for {path}: {error}");
                    }
                }
            }

            if !perf_out.is_empty() {
                for (i, line) in perf_out.lines().enumerate() {
                    if i < 80 {
                        report.call_graph_summary.push_str(line);
                        report.call_graph_summary.push('\n');
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    // Parse lines like: "     8.50%  my_app  [.] my_function"
                    if let Some(pct_pos) = trimmed.find('%') {
                        if pct_pos < 10 {
                            if let Ok(pct) = trimmed[..pct_pos].trim().parse::<f64>() {
                                let sym_pos = trimmed.find("[.]").or_else(|| trimmed.find("[k]"));
                                if let Some(pos) = sym_pos {
                                    let sym = trimmed[pos + 3..].trim().to_string();
                                    if !sym.is_empty() {
                                        report.top_symbols.push((sym, pct));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    report
}

pub fn serialize_profile_to_json(report: &ProfileReport) -> String {
    let top_symbols: Vec<serde_json::Value> = report
        .top_symbols
        .iter()
        .map(|(sym, pct)| serde_json::json!({ "symbol": sym, "overhead_percent": pct }))
        .collect();

    let active_stacks: Vec<serde_json::Value> = report
        .active_stacks
        .iter()
        .map(|st| {
            serde_json::json!({
                "tid": st.tid,
                "frames": st.frames,
            })
        })
        .collect();

    serde_json::json!({
        "perf_available": report.perf_available,
        "top_symbols": top_symbols,
        "call_graph_summary": report.call_graph_summary,
        "active_stacks": active_stacks,
    })
    .to_string()
}
