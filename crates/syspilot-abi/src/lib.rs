#![forbid(unsafe_code)]
//! Kernel/userspace event ABI.
//!
//! This crate deliberately has no allocator-dependent types and no external
//! dependencies. eBPF programs and userspace collectors communicate only with
//! these fixed-size, versioned records.

/// Bump only for an incompatible wire-format change.
pub const ABI_VERSION: u16 = 1;
pub const RAW_EVENT_HEADER_SIZE: usize = 40;
pub const RAW_PROCESS_EXIT_SIZE: usize = 56;
pub const RAW_SIGNAL_GENERATED_SIZE: usize = 64;
pub const RAW_OOM_VICTIM_SIZE: usize = 48;
pub const RAW_TELEMETRY_LOSS_SIZE: usize = 48;

/// Identity is stable for one boot and is never represented by PID alone.
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct ProcessIdentity {
    pub tgid: u32,
    pub start_boottime_ns: u64,
}

/// Event tag written by eBPF before the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EventKind {
    ProcessExit = 1,
    SignalGenerated = 2,
    OomVictim = 3,
    TelemetryLoss = 4,
}

impl EventKind {
    pub const fn from_raw(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::ProcessExit),
            2 => Some(Self::SignalGenerated),
            3 => Some(Self::OomVictim),
            4 => Some(Self::TelemetryLoss),
            _ => None,
        }
    }
}

/// Shared prefix for all raw BPF ring-buffer records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct RawEventHeader {
    pub abi_version: u16,
    pub kind: u16,
    pub size: u32,
    pub timestamp_ns: u64,
    pub tgid: u32,
    pub pid: u32,
    pub start_boottime_ns: u64,
    pub cgroup_id: u64,
}

/// Raw payload for `EventKind::ProcessExit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct RawProcessExit {
    pub header: RawEventHeader,
    pub ppid: u32,
    pub exit_code: i32,
    /// Bit 0 indicates the exiting task is the thread-group leader.
    pub flags: u32,
    /// Explicit C tail padding; must be zeroed by the producer.
    pub reserved: u32,
}

/// Raw payload for `EventKind::SignalGenerated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct RawSignalGenerated {
    pub header: RawEventHeader,
    pub signal: i32,
    pub si_code: i32,
    pub sender_pid: u32,
    pub sender_uid: u32,
    /// Bit 0 means the kernel, rather than a process, generated the signal.
    pub flags: u32,
    pub reserved: u32,
}

/// Raw payload for `EventKind::OomVictim`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct RawOomVictim {
    pub header: RawEventHeader,
    /// 1 = global OOM, 2 = memcg OOM, 0 = collector cannot determine it.
    pub oom_kind: u32,
    pub reserved: u32,
}

/// Raw payload for `EventKind::TelemetryLoss`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct RawTelemetryLoss {
    pub header: RawEventHeader,
    pub dropped_records: u64,
}

/// Decoded fixed-size record. It contains no references or heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEvent {
    ProcessExit(RawProcessExit),
    SignalGenerated(RawSignalGenerated),
    OomVictim(RawOomVictim),
    TelemetryLoss(RawTelemetryLoss),
}

impl RawEventHeader {
    /// Decodes an explicitly native-endian BPF record without pointer casts.
    ///
    /// Linux BPF ring-buffer consumers run on the same host as producers, so
    /// native endianness is intentional. Cross-host export uses a different,
    /// explicitly encoded protocol in a cold-path crate.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < RAW_EVENT_HEADER_SIZE {
            return None;
        }
        let header = Self {
            abi_version: u16::from_ne_bytes(bytes[0..2].try_into().ok()?),
            kind: u16::from_ne_bytes(bytes[2..4].try_into().ok()?),
            size: u32::from_ne_bytes(bytes[4..8].try_into().ok()?),
            timestamp_ns: u64::from_ne_bytes(bytes[8..16].try_into().ok()?),
            tgid: u32::from_ne_bytes(bytes[16..20].try_into().ok()?),
            pid: u32::from_ne_bytes(bytes[20..24].try_into().ok()?),
            start_boottime_ns: u64::from_ne_bytes(bytes[24..32].try_into().ok()?),
            cgroup_id: u64::from_ne_bytes(bytes[32..40].try_into().ok()?),
        };
        (header.abi_version == ABI_VERSION
            && EventKind::from_raw(header.kind).is_some()
            && header.size as usize >= RAW_EVENT_HEADER_SIZE
            && header.size as usize <= bytes.len())
        .then_some(header)
    }

    pub const fn identity(self) -> ProcessIdentity {
        ProcessIdentity {
            tgid: self.tgid,
            start_boottime_ns: self.start_boottime_ns,
        }
    }
}

impl RawProcessExit {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let header = RawEventHeader::decode(bytes)?;
        if header.kind != EventKind::ProcessExit as u16
            || header.size as usize != RAW_PROCESS_EXIT_SIZE
            || bytes.len() < RAW_PROCESS_EXIT_SIZE
        {
            return None;
        }
        Some(Self {
            header,
            ppid: u32::from_ne_bytes(bytes[40..44].try_into().ok()?),
            exit_code: i32::from_ne_bytes(bytes[44..48].try_into().ok()?),
            flags: u32::from_ne_bytes(bytes[48..52].try_into().ok()?),
            reserved: u32::from_ne_bytes(bytes[52..56].try_into().ok()?),
        })
    }

    pub const fn is_group_leader(self) -> bool {
        self.flags & 1 != 0
    }
}

impl RawSignalGenerated {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let header = RawEventHeader::decode(bytes)?;
        if header.kind != EventKind::SignalGenerated as u16
            || header.size as usize != RAW_SIGNAL_GENERATED_SIZE
            || bytes.len() < RAW_SIGNAL_GENERATED_SIZE
        {
            return None;
        }
        Some(Self {
            header,
            signal: i32::from_ne_bytes(bytes[40..44].try_into().ok()?),
            si_code: i32::from_ne_bytes(bytes[44..48].try_into().ok()?),
            sender_pid: u32::from_ne_bytes(bytes[48..52].try_into().ok()?),
            sender_uid: u32::from_ne_bytes(bytes[52..56].try_into().ok()?),
            flags: u32::from_ne_bytes(bytes[56..60].try_into().ok()?),
            reserved: u32::from_ne_bytes(bytes[60..64].try_into().ok()?),
        })
    }

    pub const fn sender_is_kernel(self) -> bool {
        self.flags & 1 != 0
    }
}

impl RawOomVictim {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let header = RawEventHeader::decode(bytes)?;
        if header.kind != EventKind::OomVictim as u16
            || header.size as usize != RAW_OOM_VICTIM_SIZE
            || bytes.len() < RAW_OOM_VICTIM_SIZE
        {
            return None;
        }
        Some(Self {
            header,
            oom_kind: u32::from_ne_bytes(bytes[40..44].try_into().ok()?),
            reserved: u32::from_ne_bytes(bytes[44..48].try_into().ok()?),
        })
    }
}

impl RawTelemetryLoss {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let header = RawEventHeader::decode(bytes)?;
        if header.kind != EventKind::TelemetryLoss as u16
            || header.size as usize != RAW_TELEMETRY_LOSS_SIZE
            || bytes.len() < RAW_TELEMETRY_LOSS_SIZE
        {
            return None;
        }
        Some(Self {
            header,
            dropped_records: u64::from_ne_bytes(bytes[40..48].try_into().ok()?),
        })
    }
}

impl RawEvent {
    /// Strictly validates ABI version, event tag, declared size, and payload.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match EventKind::from_raw(RawEventHeader::decode(bytes)?.kind)? {
            EventKind::ProcessExit => RawProcessExit::decode(bytes).map(Self::ProcessExit),
            EventKind::SignalGenerated => {
                RawSignalGenerated::decode(bytes).map(Self::SignalGenerated)
            }
            EventKind::OomVictim => RawOomVictim::decode(bytes).map(Self::OomVictim),
            EventKind::TelemetryLoss => RawTelemetryLoss::decode(bytes).map(Self::TelemetryLoss),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_exit_round_trip_decodes_without_allocation() {
        let mut bytes = [0_u8; RAW_PROCESS_EXIT_SIZE];
        bytes[0..2].copy_from_slice(&ABI_VERSION.to_ne_bytes());
        bytes[2..4].copy_from_slice(&(EventKind::ProcessExit as u16).to_ne_bytes());
        bytes[4..8].copy_from_slice(&(RAW_PROCESS_EXIT_SIZE as u32).to_ne_bytes());
        bytes[8..16].copy_from_slice(&10_u64.to_ne_bytes());
        bytes[16..20].copy_from_slice(&42_u32.to_ne_bytes());
        bytes[20..24].copy_from_slice(&43_u32.to_ne_bytes());
        bytes[24..32].copy_from_slice(&99_u64.to_ne_bytes());
        bytes[32..40].copy_from_slice(&7_u64.to_ne_bytes());
        bytes[40..44].copy_from_slice(&1_u32.to_ne_bytes());
        bytes[44..48].copy_from_slice(&9_i32.to_ne_bytes());
        bytes[48..52].copy_from_slice(&1_u32.to_ne_bytes());

        let event = RawProcessExit::decode(&bytes).expect("valid fixed record");
        assert_eq!(event.header.identity().tgid, 42);
        assert_eq!(event.exit_code, 9);
        assert!(event.is_group_leader());
    }

    #[test]
    fn invalid_version_and_truncated_payload_are_rejected() {
        assert!(RawEventHeader::decode(&[0; RAW_EVENT_HEADER_SIZE - 1]).is_none());
        let mut bytes = [0_u8; RAW_EVENT_HEADER_SIZE];
        bytes[0..2].copy_from_slice(&99_u16.to_ne_bytes());
        assert!(RawEventHeader::decode(&bytes).is_none());
    }

    #[test]
    fn rust_layout_matches_the_explicit_bpf_abi() {
        assert_eq!(std::mem::size_of::<RawEventHeader>(), RAW_EVENT_HEADER_SIZE);
        assert_eq!(std::mem::size_of::<RawProcessExit>(), RAW_PROCESS_EXIT_SIZE);
        assert_eq!(
            std::mem::size_of::<RawSignalGenerated>(),
            RAW_SIGNAL_GENERATED_SIZE
        );
        assert_eq!(std::mem::size_of::<RawOomVictim>(), RAW_OOM_VICTIM_SIZE);
        assert_eq!(
            std::mem::size_of::<RawTelemetryLoss>(),
            RAW_TELEMETRY_LOSS_SIZE
        );
    }
}
