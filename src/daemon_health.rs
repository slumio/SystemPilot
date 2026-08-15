//! Typed daemon-health filesystem adapter shared by CLI, doctor, and fleet.
use crate::distributed::ExporterHealth;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonHealthV1 {
    pub state: String,
    pub pid: u32,
    pub heartbeat_unix_nanos: u64,
    pub socket: String,
    pub netlink_state: String,
    pub dropped_events: u64,
    pub exporter: ExporterHealth,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthSnapshotV1 {
    pub health: DaemonHealthV1,
    pub heartbeat_age_ms: u128,
    pub fresh: bool,
}

pub fn read(path: &Path) -> AppResult<HealthSnapshotV1> {
    let bytes =
        fs::read(path).map_err(|error| AppError::io("could not read daemon health", error))?;
    let health = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Protocol(format!("malformed daemon health: {error}")))?;
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| AppError::io("could not inspect daemon health timestamp", error))?;
    let age = SystemTime::now()
        .duration_since(modified)
        .map_err(|error| {
            AppError::Validation(format!("daemon health timestamp is in the future: {error}"))
        })?;
    Ok(HealthSnapshotV1 {
        health,
        heartbeat_age_ms: age.as_millis(),
        fresh: age.as_secs() <= 3,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_health_is_never_defaulted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("health.json");
        fs::write(&path, b"{}").unwrap();
        assert!(read(&path).unwrap_err().to_string().contains("malformed"));
    }
}
