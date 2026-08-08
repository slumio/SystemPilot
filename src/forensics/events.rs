use serde::Serialize;
pub use syspilot_abi::ProcessIdentity;

/// Compact, allocation-free records accepted from a kernel collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelEvent {
    ProcessExit {
        timestamp_ns: u64,
        identity: ProcessIdentity,
        pid: u32,
        ppid: u32,
        cgroup_id: u64,
        exit_code: i32,
        is_group_leader: bool,
    },
    SignalGenerated {
        timestamp_ns: u64,
        target: ProcessIdentity,
        signal: i32,
        si_code: i32,
        sender_pid: u32,
        sender_uid: u32,
        sender_is_kernel: bool,
        cgroup_id: u64,
    },
    OomVictim {
        timestamp_ns: u64,
        victim: ProcessIdentity,
        cgroup_id: u64,
        kind: OomKind,
    },
    /// A loss record prevents absent telemetry becoming false negative evidence.
    TelemetryLoss {
        timestamp_ns: u64,
        dropped_records: u64,
    },
}

impl KernelEvent {
    pub fn timestamp_ns(self) -> u64 {
        match self {
            Self::ProcessExit { timestamp_ns, .. }
            | Self::SignalGenerated { timestamp_ns, .. }
            | Self::OomVictim { timestamp_ns, .. }
            | Self::TelemetryLoss { timestamp_ns, .. } => timestamp_ns,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OomKind {
    Global,
    Memcg,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExitDisposition {
    Exited { status: u8 },
    Signalled { signal: u8, core_dumped: bool },
}

impl ExitDisposition {
    pub fn from_exit_code(exit_code: i32) -> Self {
        let raw = exit_code as u32;
        let signal = (raw & 0x7f) as u8;
        if signal == 0 {
            Self::Exited {
                status: ((raw >> 8) & 0xff) as u8,
            }
        } else {
            Self::Signalled {
                signal,
                core_dumped: raw & 0x80 != 0,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Confidence {
    Unknown,
    Low,
    Medium,
    High,
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SignalOrigin {
    Kernel,
    Process { pid: u32, uid: u32 },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Cause {
    NormalExit { status: u8 },
    FatalSignal { signal: u8, origin: SignalOrigin },
    GlobalOom,
    CgroupOom { cgroup_id: u64 },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Evidence {
    Exit {
        disposition: ExitDisposition,
    },
    Signal {
        signal: i32,
        origin: SignalOrigin,
        si_code: i32,
    },
    OomVictim {
        kind: OomKind,
        cgroup_id: u64,
    },
    TelemetryLoss {
        dropped_records: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeathReport {
    pub identity: ProcessIdentity,
    pub timestamp_ns: u64,
    pub cgroup_id: u64,
    pub disposition: ExitDisposition,
    pub cause: Cause,
    pub confidence: Confidence,
    pub evidence: Vec<Evidence>,
    pub evidence_incomplete: bool,
}

/// Converts a validated fixed-size ABI event into the forensic domain model.
///
/// This is the collector-side hot path: it performs no allocation, I/O,
/// formatting, locking, or filesystem access.
pub fn normalize_raw_event(raw: syspilot_abi::RawEvent) -> KernelEvent {
    match raw {
        syspilot_abi::RawEvent::ProcessExit(event) => KernelEvent::ProcessExit {
            timestamp_ns: event.header.timestamp_ns,
            identity: event.header.identity(),
            pid: event.header.pid,
            ppid: event.ppid,
            cgroup_id: event.header.cgroup_id,
            exit_code: event.exit_code,
            is_group_leader: event.is_group_leader(),
        },
        syspilot_abi::RawEvent::SignalGenerated(event) => KernelEvent::SignalGenerated {
            timestamp_ns: event.header.timestamp_ns,
            target: event.header.identity(),
            signal: event.signal,
            si_code: event.si_code,
            sender_pid: event.sender_pid,
            sender_uid: event.sender_uid,
            sender_is_kernel: event.sender_is_kernel(),
            cgroup_id: event.header.cgroup_id,
        },
        syspilot_abi::RawEvent::OomVictim(event) => KernelEvent::OomVictim {
            timestamp_ns: event.header.timestamp_ns,
            victim: event.header.identity(),
            cgroup_id: event.header.cgroup_id,
            kind: match event.oom_kind {
                1 => OomKind::Global,
                2 => OomKind::Memcg,
                _ => OomKind::Unknown,
            },
        },
        syspilot_abi::RawEvent::TelemetryLoss(event) => KernelEvent::TelemetryLoss {
            timestamp_ns: event.header.timestamp_ns,
            dropped_records: event.dropped_records,
        },
    }
}
