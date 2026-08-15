use crate::causal_engine::CausalGraph;
use crate::config;
use crate::telemetry::{self, SystemTelemetry};
use crate::ui::streamer::MdStreamer;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Terminal,
};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static DAEMON_DEGRADATION_REPORTED: AtomicBool = AtomicBool::new(false);

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

fn enable_raw_mode() -> io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    Ok(())
}

fn disable_raw_mode() -> io::Result<()> {
    let raw_result = crossterm::terminal::disable_raw_mode();
    let display_result = crossterm::execute!(
        io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
        crossterm::style::ResetColor
    );
    raw_result.and(display_result)
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(error) = disable_raw_mode() {
            eprintln!("terminal restoration failed: {error}");
        }
    }
}

fn get_terminal_size() -> (u16, u16) {
    crossterm::terminal::size()
        .map(|(columns, rows)| (columns.max(40), rows.max(10)))
        .unwrap_or((80, 24))
}

fn signal_process(pid: i32, signal: nix::sys::signal::Signal) {
    if let Err(error) = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal) {
        eprintln!("{signal:?} failed for PID {pid}: {error}");
    }
}

fn wait_for_enter() -> io::Result<()> {
    let mut input = String::new();
    match io::stdin().read_line(&mut input)? {
        0 => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "terminal input closed",
        )),
        _ => Ok(()),
    }
}

fn query_daemon() -> Option<serde_json::Value> {
    match crate::daemon_client::process_tree() {
        Ok(response) => {
            DAEMON_DEGRADATION_REPORTED.store(false, Ordering::Relaxed);
            Some(response)
        }
        Err(error) => {
            if !DAEMON_DEGRADATION_REPORTED.swap(true, Ordering::Relaxed) {
                eprintln!("DEGRADED cause=\"{error}\" impact=\"monitor may miss daemon lifecycle data\" fallback=\"bounded procfs refresh\" recovery=\"start `syspilot daemon` and retry\"");
            }
            None
        }
    }
}

fn get_processes(history: &mut HashMap<i32, HistoryData>) -> Vec<TuiProcess> {
    let mut list = Vec::new();
    let now = Instant::now();
    let clk_tck = nix::unistd::sysconf(nix::unistd::SysconfVar::CLK_TCK)
        .ok()
        .flatten()
        .unwrap_or(100) as f64;
    let clk_tck = if clk_tck <= 0.0 { 100.0 } else { clk_tck };

    // Fast path: read from daemon
    if let Some(j) = query_daemon() {
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
                                tp.cpu_usage_pct =
                                    ((total_ticks.saturating_sub(h.cpu_ticks)) as f64 / clk_tck)
                                        / elapsed
                                        * 100.0;
                                tp.read_rate_kb = (read_bytes.saturating_sub(h.read_bytes)) as f64
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

    let snapshot = crate::proc_snapshot::ProcSnapshot::shared(Duration::from_millis(250));
    for pt in &snapshot.processes {
        let pid = pt.pid;
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
                    tp.cpu_usage_pct = (total_ticks.saturating_sub(h.cpu_ticks) as f64 / clk_tck)
                        / elapsed
                        * 100.0;
                    tp.read_rate_kb =
                        (pt.read_bytes.saturating_sub(h.read_bytes)) as f64 / 1024.0 / elapsed;
                    tp.write_rate_kb =
                        (pt.write_bytes.saturating_sub(h.write_bytes)) as f64 / 1024.0 / elapsed;
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
    list
}

fn render_frame<B: Backend>(
    terminal: &mut Terminal<B>,
    processes: &[TuiProcess],
    selected: usize,
    scroll: usize,
    sort_col: usize,
    st: &SystemTelemetry,
) {
    let result = terminal.draw(|frame| {
        let areas = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(frame.area());
        let used_mb = (st.mem_total_kb.saturating_sub(st.mem_available_kb)) / 1024;
        let total_mb = st.mem_total_kb / 1024;
        let sort = ["CPU%", "I/O Rate", "PID"][sort_col.min(2)];
        frame.render_widget(
            Paragraph::new(format!(
                "SysPilot Monitor | Load: {} | Mem: {used_mb}MB / {total_mb}MB | Sort: {sort}",
                st.load_avg
            ))
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::ALL)),
            areas[0],
        );
        let rows = processes.iter().map(|process| {
            let style = if process.is_anomalous {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else if process.state == "R" {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(process.pid.to_string()),
                Cell::from(process.ppid.to_string()),
                Cell::from(process.state.as_str()),
                Cell::from(format!("{:.1}%", process.cpu_usage_pct)),
                Cell::from(format!("{:.1} KB/s", process.read_rate_kb)),
                Cell::from(format!("{:.1} KB/s", process.write_rate_kb)),
                Cell::from(process.name.as_str()),
            ])
            .style(style)
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(7),
                Constraint::Length(9),
                Constraint::Length(14),
                Constraint::Length(14),
                Constraint::Min(12),
            ],
        )
        .header(
            Row::new([
                "PID",
                "PPID",
                "STATE",
                "CPU",
                "DISK READ",
                "DISK WRITE",
                "PROCESS",
            ])
            .style(
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ")
        .block(Block::default().borders(Borders::ALL));
        let mut state =
            TableState::default().with_selected((!processes.is_empty()).then_some(selected));
        *state.offset_mut() = scroll;
        frame.render_stateful_widget(table, areas[1], &mut state);
        frame.render_widget(
            Paragraph::new(
                "[Tab] Sort  [e/Enter] Explain  [s] Stop  [r] Continue  [x] Kill  [q] Quit",
            ),
            areas[2],
        );
    });
    if let Err(error) = result {
        eprintln!("terminal render failed: {error}");
    }
}

pub fn run_monitor() {
    let _terminal_guard = match TerminalGuard::enter() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("❌ Restricted terminal: {error}. Recovery: run diagnostics without `monitor` from this terminal.");
            return;
        }
    };
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("terminal initialization failed: {error}");
            return;
        }
    };

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
        let (_, height) = get_terminal_size();
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
            render_frame(&mut terminal, &processes, selected, scroll, sort_col, &st);
            needs_render = false;
        }

        match crossterm::event::poll(Duration::from_millis(100)) {
            Ok(false) => continue,
            Err(error) => {
                eprintln!("terminal input polling failed: {error}");
                break;
            }
            Ok(true) => {}
        }
        let event = match crossterm::event::read() {
            Ok(event) => event,
            Err(error) => {
                eprintln!("terminal input failed or reached EOF: {error}");
                break;
            }
        };
        if matches!(event, crossterm::event::Event::Resize(_, _)) {
            needs_render = true;
            continue;
        }
        if let crossterm::event::Event::Key(key) = event {
            use crossterm::event::{KeyCode, KeyEventKind};
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let c = match key.code {
                KeyCode::Char(character) if character.is_ascii() => character as u8,
                KeyCode::Tab => b'\t',
                KeyCode::Enter => b'\n',
                KeyCode::Up => b'k',
                KeyCode::Down => b'j',
                KeyCode::Esc => b'q',
                _ => continue,
            };
            match c {
                b'q' => break,
                b'\t' => {
                    sort_col = (sort_col + 1) % 3;
                    last_refresh = Instant::now() - Duration::from_secs(2);
                    needs_render = true;
                }
                b'j' if selected + 1 < processes.len() => {
                    selected += 1;
                    needs_render = true;
                }
                b'k' if selected > 0 => {
                    selected -= 1;
                    needs_render = true;
                }
                b's' if !processes.is_empty() => {
                    signal_process(processes[selected].pid, nix::sys::signal::Signal::SIGSTOP)
                }
                b'r' if !processes.is_empty() => {
                    signal_process(processes[selected].pid, nix::sys::signal::Signal::SIGCONT)
                }
                b'x' if !processes.is_empty() => {
                    signal_process(processes[selected].pid, nix::sys::signal::Signal::SIGKILL)
                }
                b'e' | b'\n' if !processes.is_empty() => {
                    let target_pid = processes[selected].pid;
                    let target_name = processes[selected].name.clone();

                    if let Err(error) = disable_raw_mode() {
                        eprintln!("❌ Could not suspend monitor terminal mode: {error}");
                        break;
                    }
                    println!(
                        "🧠 Querying SysPilot AI diagnostic explanation for PID {} ({})...\n",
                        target_pid, target_name
                    );

                    let conf = match config::load_checked() {
                        Ok(config) => config,
                        Err(error) => {
                            eprintln!("Configuration error: {error}. Run `syspilot doctor` and correct the reported file; AI explanation was not started.");
                            println!("Press Enter to return to monitor...");
                            if let Err(error) = wait_for_enter() {
                                eprintln!("terminal input failed: {error}");
                                break;
                            }
                            if let Err(error) = enable_raw_mode() {
                                eprintln!("❌ Could not resume monitor terminal mode: {error}");
                                break;
                            }
                            needs_render = true;
                            continue;
                        }
                    };
                    let mut graph = CausalGraph::new();
                    graph.build_graph(2, false, target_pid);
                    let node_id = format!("pid:{}", target_pid);
                    let path = graph.trace_root_cause(&node_id);
                    let chain = graph.serialize_chain_to_json(&path);

                    let causal_chain = match serde_json::from_str::<serde_json::Value>(&chain) {
                        Ok(value) => value,
                        Err(error) => {
                            eprintln!("causal graph serialization failed: {error}");
                            break;
                        }
                    };
                    let ctx = serde_json::json!({
                        "current_dir": crate::utils::run_command_output("pwd").0.trim().to_string(),
                        "analysis_type": "causal_inference_diagnostics",
                        "target_process": target_name,
                        "target_pid": target_pid,
                        "causal_chain": causal_chain,
                    });

                    let context = match serde_json::to_string_pretty(&ctx) {
                        Ok(context) => context,
                        Err(error) => {
                            eprintln!("diagnostic context serialization failed: {error}");
                            break;
                        }
                    };
                    let prompt = format!(
                                "You are a senior system reliability engineer performing root-cause analysis.\n\
                                Here is the structured JSON representation of the traversed causal path:\n{}\n\n\
                                Please explain the diagnostic findings, step-by-step root cause chain, and action recommendations.",
                                context
                            );

                    let mut streamer = MdStreamer::new();
                    crate::ai::query_ai_stream(&conf, &prompt, &mut streamer);

                    println!("\n{}\nPress Enter to return to monitor...", "-".repeat(60));
                    if let Err(error) = wait_for_enter() {
                        eprintln!("terminal input failed: {error}");
                        break;
                    }

                    if let Err(error) = enable_raw_mode() {
                        eprintln!("❌ Could not resume monitor terminal mode: {error}");
                        break;
                    }
                    last_refresh = Instant::now() - Duration::from_secs(2);
                    needs_render = true;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn process(pid: i32) -> TuiProcess {
        TuiProcess {
            pid,
            ppid: 1,
            name: format!("worker-{pid}"),
            state: "S".into(),
            cpu_usage_pct: 1.5,
            read_rate_kb: 2.0,
            write_rate_kb: 3.0,
            is_anomalous: false,
        }
    }

    #[test]
    fn ten_thousand_processes_render_with_bounded_work_and_resize() {
        let processes: Vec<_> = (1..=10_000).map(process).collect();
        let mut terminal = Terminal::new(TestBackend::new(160, 50)).unwrap();
        let started = Instant::now();
        render_frame(
            &mut terminal,
            &processes,
            9_999,
            9_950,
            0,
            &SystemTelemetry::default(),
        );
        assert!(started.elapsed() < Duration::from_secs(2));

        terminal.backend_mut().resize(80, 24);
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, 80, 24))
            .unwrap();
        render_frame(
            &mut terminal,
            &processes,
            0,
            0,
            1,
            &SystemTelemetry::default(),
        );
    }
}
