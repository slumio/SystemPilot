use crate::causal_engine::CausalGraph;
use crate::config;
use crate::telemetry::{self, SystemTelemetry};
use crate::ui::streamer::MdStreamer;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct TuiProcess {
    pid: i32,
    ppid: i32,
    name: String,
    state: String,
    cpu_usage_pct: f64,
    read_rate_kb: f64,
    write_rate_kb: f64,
    is_anomalous: bool,
}

#[derive(Default)]
struct HistoryData {
    cpu_ticks: u64,
    read_bytes: u64,
    write_bytes: u64,
    last_time: Option<Instant>,
}

fn enable_raw_mode() {
    // Use stty to enable raw mode without ncurses
    let _ = std::process::Command::new("stty")
        .args(["-echo", "cbreak"])
        .status();
    // Hide cursor
    print!("\x1b[?25l");
    let _ = io::stdout().flush();
}

fn disable_raw_mode() {
    // Restore terminal
    let _ = std::process::Command::new("stty")
        .args(["echo", "-cbreak"])
        .status();
    // Show cursor, reset colours
    print!("\x1b[?25h\x1b[0m");
    let _ = io::stdout().flush();
}

fn get_terminal_size() -> (u16, u16) {
    // Use tput as a portable fallback
    let cols = std::process::Command::new("tput")
        .arg("cols")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(80u16);
    let rows = std::process::Command::new("tput")
        .arg("lines")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(24u16);
    (cols.max(40), rows.max(10))
}

fn query_daemon() -> Option<String> {
    let mut stream = crate::config::connect_daemon().ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    stream.write_all(b"{\"request\":\"process_tree\"}").ok()?;
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    if buf.is_empty() {
        None
    } else {
        Some(buf)
    }
}

fn get_processes(history: &mut HashMap<i32, HistoryData>) -> Vec<TuiProcess> {
    let mut list = Vec::new();
    let now = Instant::now();
    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let clk_tck = if clk_tck <= 0.0 { 100.0 } else { clk_tck };

    // Fast path: read from daemon
    if let Some(res) = query_daemon() {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&res) {
            if j["status"].as_str() == Some("ok") {
                if let Some(procs) = j["processes"].as_array() {
                    for p in procs {
                        let pid = p["pid"].as_i64().unwrap_or(0) as i32;
                        let mut tp = TuiProcess {
                            pid,
                            ppid: p["ppid"].as_i64().unwrap_or(0) as i32,
                            name: p["name"].as_str().unwrap_or("").to_string(),
                            state: p["state"].as_str().unwrap_or("S").to_string(),
                            cpu_usage_pct: 0.0,
                            read_rate_kb: 0.0,
                            write_rate_kb: 0.0,
                            is_anomalous: false,
                        };

                        let mut utime = 0u64;
                        let mut stime = 0u64;
                        let mut read_bytes = 0u64;
                        let mut write_bytes = 0u64;

                        if let Ok(stat) = fs::read_to_string(format!("/proc/{}/stat", pid)) {
                            if let Some(rp) = stat.rfind(')') {
                                let fields: Vec<&str> = stat[rp + 2..].split_whitespace().collect();
                                if fields.len() > 11 {
                                    utime = fields[11].parse().unwrap_or(0);
                                }
                                if fields.len() > 12 {
                                    stime = fields[12].parse().unwrap_or(0);
                                }
                            }
                        }

                        if let Ok(f) = fs::File::open(format!("/proc/{}/io", pid)) {
                            for line in io::BufReader::new(f).lines().map_while(Result::ok) {
                                if let Some(v) = line.strip_prefix("read_bytes:") {
                                    read_bytes = v.trim().parse().unwrap_or(0);
                                } else if let Some(v) = line.strip_prefix("write_bytes:") {
                                    write_bytes = v.trim().parse().unwrap_or(0);
                                }
                            }
                        }

                        let total_ticks = utime + stime;
                        if let Some(h) = history.get(&pid) {
                            if let Some(last) = h.last_time {
                                let elapsed = now.duration_since(last).as_secs_f64();
                                if elapsed > 0.05 {
                                    tp.cpu_usage_pct = ((total_ticks.saturating_sub(h.cpu_ticks))
                                        as f64
                                        / clk_tck)
                                        / elapsed
                                        * 100.0;
                                    tp.read_rate_kb = (read_bytes.saturating_sub(h.read_bytes))
                                        as f64
                                        / 1024.0
                                        / elapsed;
                                    tp.write_rate_kb = (write_bytes.saturating_sub(h.write_bytes))
                                        as f64
                                        / 1024.0
                                        / elapsed;
                                }
                            }
                        }
                        history.insert(
                            pid,
                            HistoryData {
                                cpu_ticks: total_ticks,
                                read_bytes,
                                write_bytes,
                                last_time: Some(now),
                            },
                        );

                        if tp.state == "D" || tp.cpu_usage_pct > 80.0 || tp.write_rate_kb > 5000.0 {
                            tp.is_anomalous = true;
                        }
                        list.push(tp);
                    }
                    return list;
                }
            }
        }
    }

    // Slow fallback: scan /proc directly
    if let Ok(dir) = fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let fname = entry.file_name();
            let pid_str = fname.to_string_lossy();
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let pid: i32 = match pid_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let pt = telemetry::collect_process_telemetry_basic(pid);
            if pt.pid == 0 {
                continue;
            }
            let mut tp = TuiProcess {
                pid,
                ppid: pt.ppid,
                name: pt.name.clone(),
                state: pt.state.clone(),
                cpu_usage_pct: 0.0,
                read_rate_kb: 0.0,
                write_rate_kb: 0.0,
                is_anomalous: false,
            };
            let total_ticks = pt.utime + pt.stime;
            if let Some(h) = history.get(&pid) {
                if let Some(last) = h.last_time {
                    let elapsed = now.duration_since(last).as_secs_f64();
                    if elapsed > 0.05 {
                        tp.cpu_usage_pct =
                            (total_ticks.saturating_sub(h.cpu_ticks) as f64 / clk_tck) / elapsed
                                * 100.0;
                        tp.read_rate_kb =
                            (pt.read_bytes.saturating_sub(h.read_bytes)) as f64 / 1024.0 / elapsed;
                        tp.write_rate_kb = (pt.write_bytes.saturating_sub(h.write_bytes)) as f64
                            / 1024.0
                            / elapsed;
                    }
                }
            }
            history.insert(
                pid,
                HistoryData {
                    cpu_ticks: total_ticks,
                    read_bytes: pt.read_bytes,
                    write_bytes: pt.write_bytes,
                    last_time: Some(now),
                },
            );
            if tp.state == "D" || tp.cpu_usage_pct > 80.0 || tp.write_rate_kb > 5000.0 {
                tp.is_anomalous = true;
            }
            list.push(tp);
        }
    }
    list
}

fn render_frame(
    width: usize,
    height: usize,
    processes: &[TuiProcess],
    selected: usize,
    scroll: usize,
    sort_col: usize,
    st: &SystemTelemetry,
) {
    let mut fb = String::with_capacity(width * height * 8);

    // Clear + home
    fb.push_str("\x1b[2J\x1b[H");

    // Top border
    fb.push_str("\x1b[90m┌");
    for _ in 0..width.saturating_sub(2) {
        fb.push('─');
    }
    fb.push_str("┐\r\n");

    // Content rows
    for _ in 0..height.saturating_sub(2) {
        fb.push('│');
        for _ in 0..width.saturating_sub(2) {
            fb.push(' ');
        }
        fb.push_str("│\r\n");
    }

    // Bottom border
    fb.push('└');
    for _ in 0..width.saturating_sub(2) {
        fb.push('─');
    }
    fb.push_str("┘\x1b[0m");

    // Header
    let used_mb = (st.mem_total_kb.saturating_sub(st.mem_available_kb)) / 1024;
    let total_mb = st.mem_total_kb / 1024;
    let header = format!(
        "🤖 SysPilot Monitor | Load: {} | Mem: {}MB / {}MB",
        st.load_avg, used_mb, total_mb
    );
    fb.push_str(&format!("\x1b[2;3H\x1b[1;36m{}\x1b[0m", header));

    let sort_label = match sort_col {
        0 => "CPU%",
        1 => "I/O Rate",
        _ => "PID",
    };
    let sort_str = format!("[Sorting by: {}]", sort_label);
    let col = width.saturating_sub(sort_str.len() + 2);
    fb.push_str(&format!("\x1b[2;{}H\x1b[1;33m{}\x1b[0m", col, sort_str));

    // Column header
    fb.push_str("\x1b[4;3H\x1b[1;90m  PID    PPID   STATE   CPU%     DISK READ     DISK WRITE    PROCESS NAME\x1b[0m");
    fb.push_str(&format!(
        "\x1b[5;3H\x1b[90m{}\x1b[0m",
        "-".repeat(width.saturating_sub(6))
    ));

    // Process list
    let list_height = height.saturating_sub(8);
    for i in 0..list_height {
        let row = 6 + i;
        let proc_idx = scroll + i;
        fb.push_str(&format!("\x1b[{};3H", row));

        if proc_idx >= processes.len() {
            fb.push_str(&" ".repeat(width.saturating_sub(6)));
            continue;
        }

        let p = &processes[proc_idx];
        let line = format!(
            "{:<8}{:<8}{:<8}{:<9}{:<14}{:<14}{}",
            p.pid,
            p.ppid,
            p.state,
            format!("{:.1}%", p.cpu_usage_pct),
            format!("{:.1} KB/s", p.read_rate_kb),
            format!("{:.1} KB/s", p.write_rate_kb),
            p.name
        );

        let avail = width.saturating_sub(6);
        let mut line = if line.len() > avail {
            line[..avail].to_string()
        } else {
            format!("{:<width$}", line, width = avail)
        };
        line.truncate(avail);

        if proc_idx == selected {
            fb.push_str("\x1b[7m");
        } else if p.is_anomalous {
            fb.push_str("\x1b[1;31m");
        } else if p.state == "R" {
            fb.push_str("\x1b[32m");
        }
        fb.push_str(&line);
        fb.push_str("\x1b[0m");
    }

    // Footer
    fb.push_str(&format!(
        "\x1b[{};3H\x1b[1;90m[Tab] Sort  [e] AI Explain  [s] SIGSTOP  [r] SIGCONT  [x] SIGKILL  [q] Quit\x1b[0m",
        height.saturating_sub(2)
    ));

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(fb.as_bytes());
    let _ = handle.flush();
}

pub fn run_monitor() {
    enable_raw_mode();

    // Set terminal to non-blocking stdin via stty
    let _ = std::process::Command::new("stty")
        .args(["-icanon", "min", "0", "time", "1"])
        .status();

    let mut sort_col = 0usize;
    let mut selected = 0usize;
    let mut scroll = 0usize;
    let mut history: HashMap<i32, HistoryData> = HashMap::new();
    let mut processes: Vec<TuiProcess> = Vec::new();
    let mut last_refresh = Instant::now() - Duration::from_secs(2);
    // Keep the monitor inexpensive: /proc collection and ANSI redraws happen
    // once per second. Input is still polled every 100 ms below.
    let mut needs_render = true;

    loop {
        let (width, height) = get_terminal_size();
        let width = width as usize;
        let height = height as usize;

        // Refresh every second
        if last_refresh.elapsed() >= Duration::from_secs(1) {
            processes = get_processes(&mut history);
            match sort_col {
                0 => processes.sort_by(|a, b| {
                    b.cpu_usage_pct
                        .partial_cmp(&a.cpu_usage_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }),
                1 => processes.sort_by(|a, b| {
                    let a_io = a.read_rate_kb + a.write_rate_kb;
                    let b_io = b.read_rate_kb + b.write_rate_kb;
                    b_io.partial_cmp(&a_io).unwrap_or(std::cmp::Ordering::Equal)
                }),
                _ => processes.sort_by_key(|p| p.pid),
            }
            last_refresh = Instant::now();
            needs_render = true;
        }

        // Clamp selection
        if !processes.is_empty() {
            if selected >= processes.len() {
                selected = processes.len() - 1;
            }
        } else {
            selected = 0;
        }

        let list_height = height.saturating_sub(8);
        if selected < scroll {
            scroll = selected;
        } else if selected >= scroll + list_height {
            scroll = selected.saturating_sub(list_height) + 1;
        }

        if needs_render {
            let st = telemetry::collect_system_telemetry();
            render_frame(width, height, &processes, selected, scroll, sort_col, &st);
            needs_render = false;
        }

        // Non-blocking read with 100ms timeout
        let mut buf = [0u8; 4];
        use std::os::unix::io::AsRawFd;
        let fd = io::stdin().as_raw_fd();
        let mut fds: libc::fd_set = unsafe { std::mem::zeroed() };
        unsafe {
            libc::FD_ZERO(&mut fds);
            libc::FD_SET(fd, &mut fds);
        }
        let mut tv = libc::timeval {
            tv_sec: 0,
            tv_usec: 100_000,
        };
        let ret = unsafe {
            libc::select(
                fd + 1,
                &mut fds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            )
        };

        if ret > 0 {
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 4) };
            if n <= 0 {
                // Closed stdin would otherwise stay readable forever and spin
                // the monitor at full CPU. A monitor needs an interactive TTY.
                break;
            }

            let c = buf[0];
            match c {
                b'q' => break,
                b'\t' => {
                    sort_col = (sort_col + 1) % 3;
                    last_refresh = Instant::now() - Duration::from_secs(2);
                    needs_render = true;
                }
                b'j'
                    if selected + 1 < processes.len() => {
                        selected += 1;
                        needs_render = true;
                    }
                b'k'
                    if selected > 0 => {
                        selected -= 1;
                        needs_render = true;
                    }
                b's'
                    if !processes.is_empty() => {
                        unsafe {
                            libc::kill(processes[selected].pid, libc::SIGSTOP);
                        }
                    }
                b'r'
                    if !processes.is_empty() => {
                        unsafe {
                            libc::kill(processes[selected].pid, libc::SIGCONT);
                        }
                    }
                b'x'
                    if !processes.is_empty() => {
                        unsafe {
                            libc::kill(processes[selected].pid, libc::SIGKILL);
                        }
                    }
                b'\x1b'
                    // Arrow key: ESC [ A/B
                    if n >= 3 && buf[1] == b'[' => {
                        match buf[2] {
                            b'A'
                                if selected > 0 => {
                                    selected -= 1;
                                    needs_render = true;
                                }
                            b'B'
                                if selected + 1 < processes.len() => {
                                    selected += 1;
                                    needs_render = true;
                                }
                            _ => {}
                        }
                    }
                b'e' | b'\n'
                    if !processes.is_empty() => {
                        let target_pid = processes[selected].pid;
                        let target_name = processes[selected].name.clone();

                        disable_raw_mode();
                        print!("\x1b[2J\x1b[H");
                        println!(
                                "🧠 \x1b[1;36mQuerying SysPilot AI diagnostic explanation for PID {} ({})...\x1b[0m\n",
                                target_pid, target_name
                            );

                        let conf = match config::load_checked() {
                            Ok(config) => config,
                            Err(error) => {
                                eprintln!("Configuration error: {error}. Run `syspilot doctor` and correct the reported file; AI explanation was not started.");
                                println!("Press Enter to return to monitor...");
                                let _ = io::stdin().read(&mut buf);
                                enable_raw_mode();
                                needs_render = true;
                                continue;
                            }
                        };
                        let mut graph = CausalGraph::new();
                        graph.build_graph(2, false, target_pid);
                        let node_id = format!("pid:{}", target_pid);
                        let path = graph.trace_root_cause(&node_id);
                        let chain = graph.serialize_chain_to_json(&path);

                        let ctx = serde_json::json!({
                            "current_dir": crate::utils::run_command_output("pwd").0.trim().to_string(),
                            "analysis_type": "causal_inference_diagnostics",
                            "target_process": target_name,
                            "target_pid": target_pid,
                            "causal_chain": serde_json::from_str::<serde_json::Value>(&chain).unwrap_or_default(),
                        });

                        let prompt = format!(
                                "You are a senior system reliability engineer performing root-cause analysis.\n\
                                Here is the structured JSON representation of the traversed causal path:\n{}\n\n\
                                Please explain the diagnostic findings, step-by-step root cause chain, and action recommendations.",
                                serde_json::to_string_pretty(&ctx).unwrap_or_default()
                            );

                        let mut streamer = MdStreamer::new();
                        crate::ai::query_ai_stream(&conf, &prompt, &mut streamer);

                        println!(
                            "\n\x1b[90m{}\nPress Enter to return to monitor...\x1b[0m",
                            "-".repeat(60)
                        );
                        let _ = io::stdin().read(&mut buf);

                        enable_raw_mode();
                        last_refresh = Instant::now() - Duration::from_secs(2);
                        needs_render = true;
                    }
                _ => {}
            }
        }
    }

    disable_raw_mode();
}
