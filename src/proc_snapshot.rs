use crate::telemetry::{self, ProcessTelemetry};
use serde::Serialize;
use std::fs;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
pub struct ProcSnapshot {
    pub captured_at_unix_nanos: u64,
    pub processes: Vec<ProcessTelemetry>,
    pub unreadable_processes: u64,
}

type CachedSnapshot = Option<(Instant, Arc<ProcSnapshot>)>;
static CACHE: OnceLock<Mutex<CachedSnapshot>> = OnceLock::new();

impl ProcSnapshot {
    pub fn shared(max_age: Duration) -> Arc<Self> {
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(guard) = cache.lock() {
            if let Some((captured, snapshot)) = guard.as_ref() {
                if captured.elapsed() <= max_age {
                    return Arc::clone(snapshot);
                }
            }
        }
        let snapshot = Arc::new(Self::collect());
        if let Ok(mut guard) = cache.lock() {
            *guard = Some((Instant::now(), Arc::clone(&snapshot)));
        }
        snapshot
    }

    pub fn collect() -> Self {
        let mut processes = Vec::with_capacity(512);
        let mut unreadable_processes = 0;
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let text = name.to_string_lossy();
                if !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    continue;
                }
                let Ok(pid) = text.parse::<i32>() else {
                    continue;
                };
                let process = telemetry::collect_process_telemetry_basic(pid);
                if process.pid == 0 {
                    unreadable_processes += 1;
                } else {
                    processes.push(process);
                }
            }
        }
        Self {
            captured_at_unix_nanos: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .unwrap_or(0),
            processes,
            unreadable_processes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_snapshot_contains_current_process_and_reuses_recent_capture() {
        let first = ProcSnapshot::shared(Duration::from_secs(1));
        let second = ProcSnapshot::shared(Duration::from_secs(1));
        assert!(Arc::ptr_eq(&first, &second));
        assert!(first
            .processes
            .iter()
            .any(|process| process.pid == std::process::id() as i32));
    }
}
