use std::path::{Path, PathBuf};

/// Startup-only kernel feature detection. This module never runs in the event
/// path; it decides which independently degradable probes may be attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityState {
    Supported,
    Degraded,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelCapabilities {
    pub btf: CapabilityState,
    pub process_exit: CapabilityState,
    pub signal_origin: CapabilityState,
    pub signal_delivery: CapabilityState,
    pub oom_victim: CapabilityState,
}

impl KernelCapabilities {
    /// Detect from explicit paths to keep production startup testable without
    /// global process state or privileged kernel access in unit tests.
    pub fn detect_at(trace_events: &Path, btf_vmlinux: &Path) -> Self {
        let btf = state_for(btf_vmlinux);
        let process_exit = state_for(&trace_events.join("sched/sched_process_exit"));
        let signal_origin = state_for(&trace_events.join("signal/signal_generate"));
        let signal_delivery = state_for(&trace_events.join("signal/signal_deliver"));
        let oom_victim = if trace_events.join("oom/mark_victim").is_dir()
            || trace_events.join("oom/oom_kill").is_dir()
        {
            CapabilityState::Supported
        } else {
            CapabilityState::Unsupported
        };
        Self {
            btf,
            process_exit,
            signal_origin,
            signal_delivery,
            oom_victim,
        }
    }

    /// Uses tracefs first and falls back to the debugfs compatibility mount.
    pub fn detect_host() -> Self {
        let trace_events = trace_events_root();
        Self::detect_at(&trace_events, Path::new("/sys/kernel/btf/vmlinux"))
    }

    /// A production collector needs a process-exit source even in degraded
    /// mode. BTF is desirable for CO-RE identity reads but not a hard gate for
    /// every tracepoint attachment.
    pub fn can_collect_deaths(&self) -> bool {
        self.process_exit == CapabilityState::Supported
    }
}

fn state_for(path: &Path) -> CapabilityState {
    if path.exists() {
        CapabilityState::Supported
    } else {
        CapabilityState::Unsupported
    }
}

fn trace_events_root() -> PathBuf {
    let tracefs = PathBuf::from("/sys/kernel/tracing/events");
    if tracefs.is_dir() {
        tracefs
    } else {
        PathBuf::from("/sys/kernel/debug/tracing/events")
    }
}
