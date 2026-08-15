/// SysPilot Daemon — syspilot daemon
///
/// Subscribes to Linux Netlink Process Connector (cn_proc) for zero-polling
/// process lifecycle events. Serves process tree and event data over a UNIX
/// socket in the configured SysPilot runtime directory.
use crate::alert::AlertStore;
use crate::distributed::{ProcessAlertEngine, TelemetryKind, TelemetryPublisher};
use dashmap::DashMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ── Data structures ───────────────────────────────────────────────────────────

static NETLINK_STATE: AtomicI32 = AtomicI32::new(0);
static DROPPED_EVENTS: AtomicU64 = AtomicU64::new(0);
static RESPONSE_WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
struct ProcessNode {
    pid: i32,
    ppid: i32,
    name: String,
    state: char,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ProcessEvent {
    timestamp_ns: u64,
    event_type: String, // "FORK", "EXEC", "EXIT"
    pid: i32,
    ppid: i32,
    name: String,
    /// Raw wait status supplied by the kernel on EXIT events.
    exit_status: Option<i32>,
    /// Signal delivered to the parent on exit (commonly SIGCHLD), retained for
    /// diagnostics but not confused with the cause of termination.
    parent_exit_signal: Option<i32>,
    exit_reason: Option<String>,
}

// ── /proc helpers ─────────────────────────────────────────────────────────────

fn read_comm(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_ppid(pid: i32) -> i32 {
    let stat = match std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    // Format: pid (name) state ppid …
    if let Some(cp) = stat.rfind(')') {
        let rest = &stat[cp + 2..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() > 1 {
            return fields[1].parse().unwrap_or(0);
        }
    }
    0
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn write_health(state: &str, publisher: &TelemetryPublisher) {
    let health_path = crate::config::daemon_health_path();
    if let Some(parent) = health_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            tracing::warn!("[daemon] could not create health directory: {}", error);
            return;
        }
    }
    let document = serde_json::json!({
        "state": state,
        "pid": std::process::id(),
        "heartbeat_unix_nanos": now_ns(),
        "socket": crate::config::daemon_socket_label(),
        "netlink_state": match NETLINK_STATE.load(Ordering::Relaxed) { 1 => "active", 2 => "degraded", _ => "starting" },
        "dropped_events": DROPPED_EVENTS.load(Ordering::Relaxed),
        "response_write_failures": RESPONSE_WRITE_FAILURES.load(Ordering::Relaxed),
        "exporter": publisher.health(),
    });
    let temporary = health_path.with_extension("tmp");
    if let Err(error) = std::fs::write(&temporary, document.to_string())
        .and_then(|_| std::fs::rename(&temporary, &health_path))
    {
        tracing::warn!("[daemon] could not update health heartbeat: {}", error);
    }
}

fn publish_cloud_health(state: &str, publisher: &TelemetryPublisher) {
    let payload = serde_json::json!({
        "state": state,
        "agent_version": env!("CARGO_PKG_VERSION"),
        "protocol_version": crate::distributed::TELEMETRY_SCHEMA_VERSION,
        "netlink_state": match NETLINK_STATE.load(Ordering::Relaxed) { 1 => "active", 2 => "degraded", _ => "starting" },
        "dropped_events": DROPPED_EVENTS.load(Ordering::Relaxed),
        "response_write_failures": RESPONSE_WRITE_FAILURES.load(Ordering::Relaxed),
        "exporter": publisher.health(),
    });
    if let Err(error) = publisher.publish(TelemetryKind::Health, &payload) {
        tracing::warn!("[telemetry] cloud health heartbeat was not persisted: {error}");
    }
}

fn describe_exit_status(status: i32) -> String {
    let signal = status & 0x7f;
    if signal != 0 {
        let core_dumped = status & 0x80 != 0;
        let suffix = if core_dumped { " (core dumped)" } else { "" };
        return format!("terminated by signal {}{}", signal, suffix);
    }

    let code = (status >> 8) & 0xff;
    if code == 0 {
        "exited normally (code 0)".to_string()
    } else {
        format!("exited with code {}", code)
    }
}

// ── Initial /proc scan ────────────────────────────────────────────────────────

fn scan_proc(tree: &DashMap<i32, ProcessNode>) {
    let dir = match std::fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in dir.flatten() {
        let fname = entry.file_name();
        let s = fname.to_string_lossy();
        if !s.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let pid: i32 = match s.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        tree.insert(
            pid,
            ProcessNode {
                pid,
                ppid: read_ppid(pid),
                name: read_comm(pid),
                state: 'S',
            },
        );
    }
    tracing::info!("[daemon] Initial scan: {} processes", tree.len());
}

// ── Netlink cn_proc listener ──────────────────────────────────────────────────

fn netlink_listener(
    tree: Arc<DashMap<i32, ProcessNode>>,
    events: Arc<crossbeam_channel::Sender<ProcessEvent>>,
    running: Arc<AtomicBool>,
    publisher: Arc<TelemetryPublisher>,
    alerts: Arc<ProcessAlertEngine>,
    alert_state: Arc<Mutex<AlertStore>>,
) {
    use libc::*;

    // socket(PF_NETLINK, SOCK_DGRAM, NETLINK_CONNECTOR)
    // SAFETY: `socket` receives constant Linux Netlink arguments and returns an owned descriptor.
    let raw_fd = unsafe { socket(PF_NETLINK, SOCK_DGRAM | SOCK_CLOEXEC, NETLINK_CONNECTOR) };
    if raw_fd < 0 {
        NETLINK_STATE.store(2, Ordering::Relaxed);
        tracing::warn!("[daemon] Netlink socket failed; serving the initial /proc snapshot without live events.");
        return;
    }
    // SAFETY: `raw_fd` was just returned by `socket`, is non-negative, and has no other owner.
    let nl_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

    // Build sockaddr_nl
    // SAFETY: all-zero is the kernel-defined initialization for sockaddr_nl padding/optional fields.
    let mut addr: sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = AF_NETLINK as u16;
    addr.nl_groups = 1; // CN_IDX_PROC bitmask
    addr.nl_pid = std::process::id();

    // SAFETY: pointers reference a live sockaddr_nl for its exact size; the descriptor is owned.
    if unsafe {
        bind(
            nl_fd.as_raw_fd(),
            &addr as *const _ as *const sockaddr,
            std::mem::size_of_val(&addr) as u32,
        )
    } < 0
    {
        NETLINK_STATE.store(2, Ordering::Relaxed);
        tracing::error!(
            "[daemon] Netlink bind failed; serving the initial /proc snapshot without live events."
        );
        return;
    }

    // Subscribe to PROC_CN_MCAST_LISTEN
    // We build the minimal nlmsghdr + cn_msg + proc_cn_mcast_op by hand
    // (Rust doesn't have linux/cn_proc.h bindings, so we use raw bytes)
    //  PROC_CN_MCAST_LISTEN = 1
    let mut sub_buf = [0u8; 40];
    // nlmsghdr: len=40, type=NLMSG_DONE(3), flags=0, seq=0, pid
    let nlmsg_len: u32 = 40;
    sub_buf[0..4].copy_from_slice(&nlmsg_len.to_ne_bytes());
    sub_buf[4..6].copy_from_slice(&(3u16).to_ne_bytes()); // NLMSG_DONE
    sub_buf[12..16].copy_from_slice(&std::process::id().to_ne_bytes());
    // cn_msg starts at offset 16: id.idx=CN_IDX_PROC(1), id.val=CN_VAL_PROC(1), len=4
    sub_buf[16..20].copy_from_slice(&(1u32).to_ne_bytes()); // idx
    sub_buf[20..24].copy_from_slice(&(1u32).to_ne_bytes()); // val
    sub_buf[28..30].copy_from_slice(&(4u16).to_ne_bytes()); // len of data
                                                            // data = PROC_CN_MCAST_LISTEN = 1
    sub_buf[36..40].copy_from_slice(&(1u32).to_ne_bytes());

    // SAFETY: all-zero is the kernel-defined initialization for sockaddr_nl padding/optional fields.
    let mut kernel_addr: sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel_addr.nl_family = AF_NETLINK as u16;
    // SAFETY: both buffers remain live for the call and their exact lengths are supplied.
    let sent = unsafe {
        sendto(
            nl_fd.as_raw_fd(),
            sub_buf.as_ptr() as *const c_void,
            sub_buf.len(),
            0,
            &kernel_addr as *const _ as *const sockaddr,
            std::mem::size_of_val(&kernel_addr) as u32,
        )
    };
    if sent != sub_buf.len() as isize {
        NETLINK_STATE.store(2, Ordering::Relaxed);
        let error = std::io::Error::last_os_error();
        tracing::warn!(
            "[daemon] Netlink Process Connector subscription failed: {}; serving the initial /proc snapshot without live events.",
            error
        );
        return;
    }

    tracing::info!("[daemon] Netlink Process Connector subscription requested");

    let mut recv_buf = [0u8; 8192];
    let mut connector_confirmed = false;

    while running.load(Ordering::Relaxed) {
        // SAFETY: recv writes at most recv_buf.len() bytes into a valid mutable buffer.
        let len = unsafe {
            recv(
                nl_fd.as_raw_fd(),
                recv_buf.as_mut_ptr() as *mut c_void,
                recv_buf.len(),
                0,
            )
        };
        if len < 0 {
            let err = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO);
            if err == EINTR {
                continue;
            }
            NETLINK_STATE.store(2, Ordering::Relaxed);
            break;
        }

        // Parse nlmsghdr chain
        let mut offset = 0usize;
        while offset + 16 <= len as usize {
            let msg_len =
                u32::from_ne_bytes(recv_buf[offset..offset + 4].try_into().unwrap_or([0; 4]))
                    as usize;
            if msg_len < 16 || offset + msg_len > len as usize {
                break;
            }
            let msg_type = u16::from_ne_bytes(
                recv_buf[offset + 4..offset + 6]
                    .try_into()
                    .unwrap_or([0; 2]),
            );
            // NLMSG_ERROR = 2; the kernel can reject subscription
            // asynchronously even when sendto itself succeeded.
            if msg_type == 2 {
                let error_code = if msg_len >= 20 {
                    i32::from_ne_bytes(
                        recv_buf[offset + 16..offset + 20]
                            .try_into()
                            .unwrap_or([0; 4]),
                    )
                } else {
                    -libc::EPROTO
                };
                if error_code != 0 {
                    let error = std::io::Error::from_raw_os_error(-error_code);
                    tracing::warn!(
                        "[daemon] Netlink Process Connector rejected the subscription: {}. Grant cap_net_admin to the binary or run the packaged system service.",
                        error
                    );
                    NETLINK_STATE.store(2, Ordering::Relaxed);
                    return;
                }
                let aligned = (msg_len + 3) & !3;
                offset += aligned.max(16);
                continue;
            }

            // cn_msg at nlmsghdr + NLMSG_HDRLEN(16)
            let cn_offset = offset + 16;
            if cn_offset + 20 > len as usize {
                break;
            }
            let cn_idx = u32::from_ne_bytes(
                recv_buf[cn_offset..cn_offset + 4]
                    .try_into()
                    .unwrap_or([0; 4]),
            );
            let cn_val = u32::from_ne_bytes(
                recv_buf[cn_offset + 4..cn_offset + 8]
                    .try_into()
                    .unwrap_or([0; 4]),
            );

            // CN_IDX_PROC=1, CN_VAL_PROC=1
            if cn_idx == 1 && cn_val == 1 {
                if !connector_confirmed {
                    NETLINK_STATE.store(1, Ordering::Relaxed);
                    tracing::info!("[daemon] Netlink Process Connector active (zero-polling)");
                    connector_confirmed = true;
                }
                // proc_event starts at cn_msg + 20 bytes (after header)
                let ev_offset = cn_offset + 20;
                if ev_offset + 4 <= len as usize {
                    let what = u32::from_ne_bytes(
                        recv_buf[ev_offset..ev_offset + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );
                    // The proc_event union begins at +16 after what/cpu/timestamp.
                    // Linux values: FORK=1, EXEC=2, EXIT=0x80000000.
                    match what {
                        1
                            // fork: parent_pid at union +0, child_pid at union +8
                            if ev_offset + 32 <= len as usize => {
                                let parent = i32::from_ne_bytes(
                                    recv_buf[ev_offset + 16..ev_offset + 20]
                                        .try_into()
                                        .unwrap_or([0; 4]),
                                );
                                let child = i32::from_ne_bytes(
                                    recv_buf[ev_offset + 24..ev_offset + 28]
                                        .try_into()
                                        .unwrap_or([0; 4]),
                                );
                                let name = read_comm(child);
                                tree.insert(
                                    child,
                                    ProcessNode {
                                        pid: child,
                                        ppid: parent,
                                        name: name.clone(),
                                        state: 'S',
                                    },
                                );
                                let event = ProcessEvent {
                                    timestamp_ns: now_ns(),
                                    event_type: "FORK".to_string(),
                                    pid: child,
                                    ppid: parent,
                                    name,
                                    exit_status: None,
                                    parent_exit_signal: None,
                                    exit_reason: None,
                                };
                                record_event(&events, &publisher, &alerts, &alert_state, event);
                            }
                        2
                            // exec
                            if ev_offset + 24 <= len as usize => {
                                let pid = i32::from_ne_bytes(
                                    recv_buf[ev_offset + 16..ev_offset + 20]
                                        .try_into()
                                        .unwrap_or([0; 4]),
                                );
                                let name = read_comm(pid);
                                tree.entry(pid)
                                    .and_modify(|n| n.name = name.clone())
                                    .or_insert(ProcessNode {
                                        pid,
                                        ppid: read_ppid(pid),
                                        name: name.clone(),
                                        state: 'S',
                                    });
                                let event = ProcessEvent {
                                    timestamp_ns: now_ns(),
                                    event_type: "EXEC".to_string(),
                                    pid,
                                    ppid: 0,
                                    name,
                                    exit_status: None,
                                    parent_exit_signal: None,
                                    exit_reason: None,
                                };
                                record_event(&events, &publisher, &alerts, &alert_state, event);
                            }
                        0x8000_0000
                            // exit
                            if ev_offset + 32 <= len as usize => {
                                let pid = i32::from_ne_bytes(
                                    recv_buf[ev_offset + 16..ev_offset + 20]
                                        .try_into()
                                        .unwrap_or([0; 4]),
                                );
                                let exit_status = i32::from_ne_bytes(
                                    recv_buf[ev_offset + 24..ev_offset + 28]
                                        .try_into()
                                        .unwrap_or([0; 4]),
                                );
                                let parent_exit_signal = i32::from_ne_bytes(
                                    recv_buf[ev_offset + 28..ev_offset + 32]
                                        .try_into()
                                        .unwrap_or([0; 4]),
                                );
                                let name = tree
                                    .remove(&pid)
                                    .map(|(_, process)| process.name)
                                    .unwrap_or_default();
                                let event = ProcessEvent {
                                    timestamp_ns: now_ns(),
                                    event_type: "EXIT".to_string(),
                                    pid,
                                    ppid: 0,
                                    name,
                                    exit_status: Some(exit_status),
                                    parent_exit_signal: Some(parent_exit_signal),
                                    exit_reason: Some(describe_exit_status(exit_status)),
                                };
                                record_event(&events, &publisher, &alerts, &alert_state, event);
                            }
                        _ => {}
                    }
                }
            }

            // Advance: NLMSG_ALIGN(msg_len)
            let aligned = (msg_len + 3) & !3;
            offset += aligned.max(16);
        }
    }
}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn record_event(
    events: &crossbeam_channel::Sender<ProcessEvent>,
    publisher: &TelemetryPublisher,
    alerts: &ProcessAlertEngine,
    alert_state: &Mutex<AlertStore>,
    event: ProcessEvent,
) {
    if let Err(error) = publisher.publish(TelemetryKind::ProcessLifecycle, &event) {
        tracing::error!("[telemetry] lifecycle event was not persisted: {error}");
    }
    for alert in alerts.evaluate(
        &event.name,
        event.pid,
        event.ppid,
        &event.event_type,
        event.timestamp_ns,
    ) {
        let transition = match alert_state.lock() {
            Ok(mut state) => state.apply(alert),
            Err(_) => {
                tracing::error!(
                    "[alert] persistent alert state lock is poisoned; transition was not evaluated"
                );
                continue;
            }
        };
        match transition {
            Ok(Some(alert)) => {
                if let Err(error) = publisher.publish(TelemetryKind::ProcessAlert, &alert) {
                    tracing::error!(
                        "[telemetry] process alert transition was not persisted: {error}"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => tracing::error!("[alert] could not persist alert transition: {error}"),
        }
    }
    if events.try_send(event).is_err() {
        DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
        tracing::warn!("[daemon] lifecycle event queue is full; event was dropped");
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn build_process_tree_json(tree: &DashMap<i32, ProcessNode>) -> String {
    let mut out = String::from("{\"status\":\"ok\",\"processes\":[");
    let mut first = true;
    for entry in tree.iter() {
        let n = entry.value();
        if !first {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"pid\":{},\"ppid\":{},\"name\":\"{}\",\"state\":\"{}\"}}",
            n.pid,
            n.ppid,
            json_escape(&n.name),
            n.state
        ));
        first = false;
    }
    out.push_str("]}");
    out
}

fn build_events_json(rx: &crossbeam_channel::Receiver<ProcessEvent>) -> String {
    let mut evts = Vec::new();
    while let Ok(e) = rx.try_recv() {
        evts.push(e);
    }
    let mut out = String::from("{\"status\":\"ok\",\"events\":[");
    let mut first = true;
    for e in &evts {
        if !first {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"time\":{},\"type\":\"{}\",\"pid\":{},\"ppid\":{},\"name\":\"{}\",\"exit_status\":{},\"parent_exit_signal\":{},\"reason\":{}}}",
            e.timestamp_ns,
            e.event_type,
            e.pid,
            e.ppid,
            json_escape(&e.name),
            e.exit_status.map(|value| value.to_string()).unwrap_or_else(|| "null".to_string()),
            e.parent_exit_signal.map(|value| value.to_string()).unwrap_or_else(|| "null".to_string()),
            e.exit_reason.as_ref().map(|value| format!("\"{}\"", json_escape(value))).unwrap_or_else(|| "null".to_string()),
        ));
        first = false;
    }
    out.push_str("]}");
    out
}

// ── UNIX socket server ────────────────────────────────────────────────────────

fn handle_client(
    mut stream: UnixStream,
    tree: Arc<DashMap<i32, ProcessNode>>,
    rx: Arc<crossbeam_channel::Receiver<ProcessEvent>>,
    active: Arc<AtomicI32>,
) {
    let mut buf = [0u8; 2048];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => {
            active.fetch_sub(1, Ordering::Relaxed);
            return;
        }
    };

    let request = String::from_utf8_lossy(&buf[..n]);
    let response = if let Ok(j) = serde_json::from_str::<serde_json::Value>(&request) {
        match j["request"].as_str().unwrap_or("") {
            "process_tree" => build_process_tree_json(&tree),
            "events" => build_events_json(&rx),
            other => format!(
                "{{\"status\":\"error\",\"message\":\"unknown: {}\"}}",
                other
            ),
        }
    } else {
        "{\"status\":\"error\",\"message\":\"invalid json\"}".to_string()
    };

    if let Err(error) = stream.write_all(response.as_bytes()) {
        RESPONSE_WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
        tracing::warn!("[daemon] response write failed: {error}");
    }
    active.fetch_sub(1, Ordering::Relaxed);
}

fn unix_socket_server(
    tree: Arc<DashMap<i32, ProcessNode>>,
    rx: Arc<crossbeam_channel::Receiver<ProcessEvent>>,
    running: Arc<AtomicBool>,
    publisher: Arc<TelemetryPublisher>,
) {
    let runtime_dir = crate::config::daemon_runtime_dir();
    if let Err(error) = std::fs::create_dir_all(&runtime_dir) {
        tracing::error!(
            "[daemon] could not create runtime directory {}: {}",
            runtime_dir.display(),
            error
        );
        return;
    }
    let socket_path = crate::config::daemon_socket_path();
    if !crate::config::daemon_socket_is_abstract() {
        if let Err(error) = std::fs::remove_file(&socket_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("[daemon] stale socket cleanup failed: {error}");
            }
        }
    }

    let socket_addr = match crate::config::daemon_socket_addr() {
        Ok(address) => address,
        Err(error) => {
            tracing::error!("[daemon] invalid socket address: {}", error);
            return;
        }
    };
    let listener = match UnixListener::bind_addr(&socket_addr) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("[daemon] bind failed: {}", e);
            return;
        }
    };
    if !crate::config::daemon_socket_is_abstract() {
        if let Err(error) = std::fs::set_permissions(
            &socket_path,
            std::os::unix::fs::PermissionsExt::from_mode(0o660),
        ) {
            tracing::error!("[daemon] could not secure socket permissions: {error}");
            return;
        }
    }

    if let Err(error) = listener.set_nonblocking(true) {
        tracing::error!("[daemon] could not configure nonblocking listener: {error}");
        return;
    }
    tracing::info!(
        "[daemon] UNIX socket listening at {}",
        crate::config::daemon_socket_label()
    );

    let active = Arc::new(AtomicI32::new(0));
    const MAX_CLIENTS: i32 = 32;
    let mut last_heartbeat = Instant::now() - Duration::from_secs(2);
    let mut last_cloud_heartbeat = Instant::now();

    while running.load(Ordering::Relaxed) {
        if last_heartbeat.elapsed() >= Duration::from_secs(1) {
            write_health("ready", &publisher);
            last_heartbeat = Instant::now();
        }
        if last_cloud_heartbeat.elapsed() >= Duration::from_secs(60) {
            publish_cloud_health("ready", &publisher);
            last_cloud_heartbeat = Instant::now();
        }
        // Use select-like timeout via accept with a 1s deadline
        match listener.accept() {
            Ok((stream, _)) => {
                if active.load(Ordering::Relaxed) >= MAX_CLIENTS {
                    tracing::warn!("[daemon] Too many clients, dropping connection.");
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let tree2 = Arc::clone(&tree);
                let rx2 = Arc::clone(&rx);
                let active2 = Arc::clone(&active);
                std::thread::spawn(move || handle_client(stream, tree2, rx2, active2));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }

    if !crate::config::daemon_socket_is_abstract() {
        if let Err(error) = std::fs::remove_file(&socket_path) {
            tracing::warn!("[daemon] shutdown socket cleanup failed: {error}");
        }
    }
    write_health("stopped", &publisher);
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run_daemon(config: crate::config::Config) -> i32 {
    let publisher = match TelemetryPublisher::from_config(&config.distributed_telemetry) {
        Ok(publisher) => Arc::new(publisher),
        Err(error) => {
            eprintln!("❌ Invalid distributed telemetry configuration: {}", error);
            return 2;
        }
    };
    let alerts =
        match ProcessAlertEngine::new(config.distributed_telemetry.process_alert_rules.clone()) {
            Ok(alerts) => Arc::new(alerts),
            Err(error) => {
                eprintln!("❌ Invalid process alert rules: {}", error);
                return 2;
            }
        };
    let alert_state = match AlertStore::open(crate::alert::default_path()) {
        Ok(state) => Arc::new(Mutex::new(state)),
        Err(error) => {
            eprintln!("❌ Could not load persistent alert state: {error}");
            return 2;
        }
    };
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .init();

    tracing::info!("✨ SysPilot Daemon starting");
    write_health("starting", &publisher);
    publish_cloud_health("starting", &publisher);

    let tree: Arc<DashMap<i32, ProcessNode>> = Arc::new(DashMap::new());
    scan_proc(&tree);

    let (tx, rx) = crossbeam_channel::bounded::<ProcessEvent>(4096);
    let rx = Arc::new(rx);

    let running = Arc::new(AtomicBool::new(true));

    let tree_nl = Arc::clone(&tree);
    let run_nl = Arc::clone(&running);
    let publisher_nl = Arc::clone(&publisher);
    let alerts_nl = Arc::clone(&alerts);
    let alert_state_nl = Arc::clone(&alert_state);
    let nl_thread = std::thread::spawn(move || {
        netlink_listener(
            tree_nl,
            Arc::new(tx),
            run_nl,
            publisher_nl,
            alerts_nl,
            alert_state_nl,
        );
    });

    let tree_sock = Arc::clone(&tree);
    let run_sock = Arc::clone(&running);
    let publisher_sock = Arc::clone(&publisher);
    let sock_thread = std::thread::spawn(move || {
        unix_socket_server(tree_sock, rx, run_sock, publisher_sock);
    });

    let mut exit = 0;
    if nl_thread.join().is_err() {
        tracing::error!("[daemon] netlink worker panicked");
        exit = 1;
    }
    if sock_thread.join().is_err() {
        tracing::error!("[daemon] socket worker panicked");
        exit = 1;
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::describe_exit_status;

    #[test]
    fn decodes_normal_exit_status() {
        assert_eq!(describe_exit_status(0), "exited normally (code 0)");
        assert_eq!(describe_exit_status(3 << 8), "exited with code 3");
    }

    #[test]
    fn decodes_signal_exit_status() {
        assert_eq!(describe_exit_status(9), "terminated by signal 9");
        assert_eq!(
            describe_exit_status(11 | 0x80),
            "terminated by signal 11 (core dumped)"
        );
    }
}
