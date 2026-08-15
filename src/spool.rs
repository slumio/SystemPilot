use crate::distributed::TelemetryEnvelope;
use crate::error::{AppError, AppResult};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, Default)]
pub struct SpoolStats {
    pub records: u64,
    pub bytes: u64,
    pub quarantined: u64,
}

#[derive(Debug)]
pub struct DiskSpool {
    root: PathBuf,
    records: PathBuf,
    quarantine: PathBuf,
    sequence_path: PathBuf,
    max_bytes: u64,
    max_age_secs: u64,
    lock: Mutex<()>,
}

impl DiskSpool {
    pub fn open(root: PathBuf, max_bytes: u64, max_age_secs: u64) -> AppResult<Self> {
        if max_bytes == 0 || max_age_secs == 0 {
            return Err(AppError::Validation(
                "spool byte and age limits must be non-zero".into(),
            ));
        }
        let spool = Self {
            records: root.join("records"),
            quarantine: root.join("quarantine"),
            sequence_path: root.join("sequence"),
            root,
            max_bytes,
            max_age_secs,
            lock: Mutex::new(()),
        };
        spool.prepare()?;
        Ok(spool)
    }

    fn prepare(&self) -> AppResult<()> {
        for directory in [&self.root, &self.records, &self.quarantine] {
            fs::create_dir_all(directory)
                .map_err(|e| AppError::io("could not create telemetry spool directory", e))?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|e| AppError::io("could not secure telemetry spool directory", e))?;
        }
        Ok(())
    }

    fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<()> {
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| AppError::io("could not create temporary spool record", e))?;
        if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
            if let Err(cleanup) = fs::remove_file(&temporary) {
                eprintln!("spool temporary-file cleanup failed: {cleanup}");
            }
            return Err(AppError::io("could not persist spool record", error));
        }
        fs::rename(&temporary, path).map_err(|e| AppError::io("could not commit spool record", e))
    }

    pub fn next_sequence(&self) -> AppResult<u64> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Protocol("telemetry spool lock poisoned".into()))?;
        let current = match fs::read_to_string(&self.sequence_path) {
            Ok(text) => text
                .trim()
                .parse::<u64>()
                .map_err(|e| AppError::Protocol(format!("invalid spool sequence state: {e}")))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(AppError::io("could not read spool sequence state", error)),
        };
        let next = current
            .checked_add(1)
            .ok_or_else(|| AppError::Protocol("telemetry sequence exhausted".into()))?;
        Self::atomic_write(&self.sequence_path, next.to_string().as_bytes())?;
        Ok(next)
    }

    pub fn append(&self, envelope: &TelemetryEnvelope) -> AppResult<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Protocol("telemetry spool lock poisoned".into()))?;
        let bytes = envelope.encode_json()?;
        let stats = self.stats_unlocked()?;
        if stats.bytes.saturating_add(bytes.len() as u64) > self.max_bytes {
            return Err(AppError::Protocol(format!(
                "telemetry spool is full ({} byte limit)",
                self.max_bytes
            )));
        }
        let path = self.records.join(format!("{:020}.json", envelope.sequence));
        if path.exists() {
            return Err(AppError::Protocol(format!(
                "telemetry sequence {} already exists in spool",
                envelope.sequence
            )));
        }
        Self::atomic_write(&path, &bytes)
    }

    pub fn pending(&self, limit: usize) -> AppResult<Vec<TelemetryEnvelope>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Protocol("telemetry spool lock poisoned".into()))?;
        let mut paths = self.record_paths()?;
        paths.sort();
        let mut records = Vec::new();
        for path in paths.into_iter().take(limit) {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(AppError::io("could not read telemetry spool record", error))
                }
            };
            match serde_json::from_slice::<TelemetryEnvelope>(&bytes).and_then(|record| {
                record.validate().map_err(|e| {
                    serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        e.to_string(),
                    ))
                })?;
                Ok(record)
            }) {
                Ok(record) => records.push(record),
                Err(error) => {
                    let name = path.file_name().ok_or_else(|| {
                        AppError::Protocol("spool record has no file name".into())
                    })?;
                    let destination = self.quarantine.join(name);
                    fs::rename(&path, &destination).map_err(|e| {
                        AppError::io("could not quarantine corrupt spool record", e)
                    })?;
                    tracing::error!(
                        "[telemetry] quarantined corrupt spool record {}: {}",
                        destination.display(),
                        error
                    );
                }
            }
        }
        Ok(records)
    }

    pub fn acknowledge(&self, records: &[TelemetryEnvelope]) -> AppResult<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Protocol("telemetry spool lock poisoned".into()))?;
        for record in records {
            let path = self.records.join(format!("{:020}.json", record.sequence));
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(AppError::Protocol(format!(
                        "acknowledged spool record is missing: {}",
                        record.message_id
                    )));
                }
                Err(error) => {
                    return Err(AppError::io(
                        "could not remove acknowledged spool record",
                        error,
                    ))
                }
            }
        }
        Ok(())
    }

    fn record_paths(&self) -> AppResult<Vec<PathBuf>> {
        let entries = fs::read_dir(&self.records)
            .map_err(|e| AppError::io("could not read telemetry spool", e))?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|e| AppError::io("could not inspect telemetry spool entry", e))?;
            if entry.path().extension().and_then(|v| v.to_str()) == Some("json") {
                paths.push(entry.path());
            }
        }
        Ok(paths)
    }

    fn stats_unlocked(&self) -> AppResult<SpoolStats> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                AppError::Validation(format!("system clock precedes Unix epoch: {error}"))
            })?
            .as_secs();
        let mut stats = SpoolStats::default();
        for path in self.record_paths()? {
            let metadata = fs::metadata(&path)
                .map_err(|e| AppError::io("could not inspect spool record", e))?;
            stats.records += 1;
            stats.bytes = stats.bytes.saturating_add(metadata.len());
            if let Ok(modified) = metadata
                .modified()
                .and_then(|v| v.duration_since(UNIX_EPOCH).map_err(std::io::Error::other))
            {
                if now.saturating_sub(modified.as_secs()) > self.max_age_secs {
                    return Err(AppError::Protocol(format!(
                        "telemetry spool contains expired record {}; operator action required",
                        path.display()
                    )));
                }
            }
        }
        let entries = fs::read_dir(&self.quarantine)
            .map_err(|e| AppError::io("could not read spool quarantine", e))?;
        for entry in entries {
            entry.map_err(|e| AppError::io("could not inspect spool quarantine", e))?;
            stats.quarantined += 1;
        }
        Ok(stats)
    }

    pub fn stats(&self) -> AppResult<SpoolStats> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| AppError::Protocol("telemetry spool lock poisoned".into()))?;
        self.stats_unlocked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distributed::{TelemetryKind, TELEMETRY_SCHEMA_VERSION};
    use std::collections::BTreeMap;

    fn envelope(sequence: u64) -> TelemetryEnvelope {
        TelemetryEnvelope {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            message_id: format!("node-{sequence}"),
            node_id: "node".into(),
            sequence,
            observed_at_unix_nanos: 1,
            kind: TelemetryKind::Health,
            payload: serde_json::json!({"ok": true}),
            attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn sequence_is_monotonic_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let spool = DiskSpool::open(directory.path().join("spool"), 1024, 60).unwrap();
        assert_eq!(spool.next_sequence().unwrap(), 1);
        drop(spool);
        let reopened = DiskSpool::open(directory.path().join("spool"), 1024, 60).unwrap();
        assert_eq!(reopened.next_sequence().unwrap(), 2);
    }

    #[test]
    fn records_are_owner_only_and_acknowledgement_removes_them() {
        let directory = tempfile::tempdir().unwrap();
        let spool = DiskSpool::open(directory.path().join("spool"), 4096, 60).unwrap();
        let record = envelope(1);
        spool.append(&record).unwrap();
        let path = spool.records.join("00000000000000000001.json");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(spool.pending(10).unwrap().len(), 1);
        spool.acknowledge(&[record]).unwrap();
        assert_eq!(spool.stats().unwrap().records, 0);
    }

    #[test]
    fn corrupt_record_is_quarantined_and_visible() {
        let directory = tempfile::tempdir().unwrap();
        let spool = DiskSpool::open(directory.path().join("spool"), 4096, 60).unwrap();
        fs::write(spool.records.join("00000000000000000001.json"), b"not-json").unwrap();
        assert!(spool.pending(10).unwrap().is_empty());
        let stats = spool.stats().unwrap();
        assert_eq!(stats.records, 0);
        assert_eq!(stats.quarantined, 1);
    }

    #[test]
    fn capacity_failure_retains_existing_record() {
        let directory = tempfile::tempdir().unwrap();
        let spool = DiskSpool::open(directory.path().join("spool"), 200, 60).unwrap();
        let record = envelope(1);
        spool.append(&record).unwrap();
        assert!(spool.append(&envelope(2)).is_err());
        assert_eq!(spool.pending(10).unwrap().len(), 1);
    }
}
