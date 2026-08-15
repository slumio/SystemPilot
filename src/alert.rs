//! Persistent deterministic alert lifecycle state.
use crate::distributed::ProcessAlert;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

pub const ALERT_STATE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlertStatus {
    Firing,
    Acknowledged,
    Resolved,
    Suppressed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRecord {
    pub instance_id: String,
    pub rule_id: String,
    pub process_name: String,
    pub status: AlertStatus,
    pub first_observed_at_unix_nanos: u64,
    pub last_observed_at_unix_nanos: u64,
    pub transition_count: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AlertStateV1 {
    schema_version: u16,
    records: BTreeMap<String, AlertRecord>,
}

pub struct AlertStore {
    path: PathBuf,
    state: AlertStateV1,
}

impl AlertStore {
    pub fn open(path: PathBuf) -> AppResult<Self> {
        let state = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<AlertStateV1>(&text).map_err(|error| {
                AppError::Protocol(format!(
                    "invalid alert state at {}: {error}",
                    path.display()
                ))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => AlertStateV1 {
                schema_version: ALERT_STATE_SCHEMA_VERSION,
                records: BTreeMap::new(),
            },
            Err(error) => return Err(AppError::io("could not read alert state", error)),
        };
        if state.schema_version != ALERT_STATE_SCHEMA_VERSION {
            return Err(AppError::Protocol(format!(
                "unsupported alert state schema {}; expected {}",
                state.schema_version, ALERT_STATE_SCHEMA_VERSION
            )));
        }
        Ok(Self { path, state })
    }

    fn persist(&self) -> AppResult<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| AppError::Validation("alert state path has no parent".into()))?;
        fs::create_dir_all(parent)
            .map_err(|e| AppError::io("could not create alert state directory", e))?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|e| AppError::io("could not secure alert state directory", e))?;
        let bytes = serde_json::to_vec_pretty(&self.state)
            .map_err(|e| AppError::Protocol(format!("could not encode alert state: {e}")))?;
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| AppError::io("could not create temporary alert state", e))?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(AppError::io("could not persist alert state", error));
        }
        fs::rename(&temporary, &self.path)
            .map_err(|e| AppError::io("could not commit alert state", e))
    }

    fn key(rule_id: &str, process_name: &str) -> String {
        format!("{rule_id}:{process_name}")
    }

    pub fn apply(&mut self, mut alert: ProcessAlert) -> AppResult<Option<ProcessAlert>> {
        let key = Self::key(&alert.rule_id, &alert.process_name);
        let desired = if alert.event_type == "EXIT" {
            AlertStatus::Resolved
        } else {
            AlertStatus::Firing
        };
        let existing = self.state.records.get(&key).cloned();
        if existing.as_ref().is_some_and(|record| {
            record.status == AlertStatus::Suppressed
                || (record.status == AlertStatus::Acknowledged && desired == AlertStatus::Firing)
        }) {
            return Ok(None);
        }
        if existing
            .as_ref()
            .is_some_and(|record| record.status == desired)
        {
            return Ok(None);
        }
        if desired == AlertStatus::Resolved && existing.is_none() {
            return Ok(None);
        }
        let previous = existing.as_ref().map(|record| record.status);
        let first = existing
            .as_ref()
            .map_or(alert.observed_at_unix_nanos, |record| {
                record.first_observed_at_unix_nanos
            });
        let transitions = existing
            .as_ref()
            .map_or(1, |record| record.transition_count.saturating_add(1));
        let instance_id = existing.as_ref().map_or_else(
            || format!("{}-{}", alert.rule_id, alert.observed_at_unix_nanos),
            |record| record.instance_id.clone(),
        );
        self.state.records.insert(
            key,
            AlertRecord {
                instance_id: instance_id.clone(),
                rule_id: alert.rule_id.clone(),
                process_name: alert.process_name.clone(),
                status: desired,
                first_observed_at_unix_nanos: first,
                last_observed_at_unix_nanos: alert.observed_at_unix_nanos,
                transition_count: transitions,
                detail: format!("{} pid={} ppid={}", alert.event_type, alert.pid, alert.ppid),
            },
        );
        self.persist()?;
        alert.instance_id = instance_id;
        alert.state = format!("{:?}", desired).to_lowercase();
        alert.previous_state = previous.map(|status| format!("{:?}", status).to_lowercase());
        Ok(Some(alert))
    }

    pub fn set_status(
        &mut self,
        instance_id: &str,
        status: AlertStatus,
        detail: String,
        observed_at: u64,
    ) -> AppResult<AlertRecord> {
        let record = self
            .state
            .records
            .values_mut()
            .find(|record| record.instance_id == instance_id)
            .ok_or_else(|| {
                AppError::Validation(format!("unknown alert instance: {instance_id}"))
            })?;
        if record.status == AlertStatus::Resolved && status == AlertStatus::Acknowledged {
            return Err(AppError::Validation(
                "resolved alerts cannot be acknowledged".into(),
            ));
        }
        record.status = status;
        record.last_observed_at_unix_nanos = observed_at;
        record.transition_count = record.transition_count.saturating_add(1);
        record.detail = detail;
        let result = record.clone();
        self.persist()?;
        Ok(result)
    }

    pub fn list(&self) -> Vec<AlertRecord> {
        self.state.records.values().cloned().collect()
    }
}

pub fn current_time_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0)
}

pub fn default_path() -> PathBuf {
    crate::config::get_syspilot_dir().join("alerts-v1.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn event(kind: &str, timestamp: u64) -> ProcessAlert {
        ProcessAlert {
            instance_id: String::new(),
            state: String::new(),
            previous_state: None,
            rule_id: "rule".into(),
            process_name: "worker".into(),
            pid: 10,
            ppid: 1,
            event_type: kind.into(),
            observed_at_unix_nanos: timestamp,
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn lifecycle_is_deduplicated_acknowledged_and_resolved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("alerts.json");
        let mut store = AlertStore::open(path.clone()).unwrap();
        let firing = store.apply(event("EXEC", 1)).unwrap().unwrap();
        assert_eq!(firing.state, "firing");
        assert!(store.apply(event("FORK", 2)).unwrap().is_none());
        store
            .set_status(
                &firing.instance_id,
                AlertStatus::Acknowledged,
                "operator".into(),
                3,
            )
            .unwrap();
        assert!(store.apply(event("EXEC", 4)).unwrap().is_none());
        let resolved = store.apply(event("EXIT", 5)).unwrap().unwrap();
        assert_eq!(resolved.state, "resolved");
        assert_eq!(resolved.previous_state.as_deref(), Some("acknowledged"));
        drop(store);
        let reopened = AlertStore::open(path).unwrap();
        assert_eq!(reopened.list()[0].status, AlertStatus::Resolved);
    }

    #[test]
    fn suppression_persists_and_blocks_automatic_transitions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("alerts.json");
        let mut store = AlertStore::open(path.clone()).unwrap();
        let firing = store.apply(event("EXEC", 1)).unwrap().unwrap();
        store
            .set_status(
                &firing.instance_id,
                AlertStatus::Suppressed,
                "maintenance".into(),
                2,
            )
            .unwrap();
        drop(store);
        let mut reopened = AlertStore::open(path).unwrap();
        assert!(reopened.apply(event("EXIT", 3)).unwrap().is_none());
        assert_eq!(reopened.list()[0].status, AlertStatus::Suppressed);
    }

    #[test]
    fn corrupt_state_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("alerts.json");
        fs::write(&path, b"not-json").unwrap();
        assert!(AlertStore::open(path).is_err());
    }
}
