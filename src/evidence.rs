use crate::error::{AppError, AppResult};
use crate::{config, forensics::KernelCapabilities, telemetry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const EVIDENCE_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_RETENTION_DAYS: u64 = 30;
pub const DEFAULT_RETENTION_BYTES: u64 = 1024 * 1024 * 1024;
static CASE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRangeV1 {
    pub started_at_unix_nanos: u64,
    pub ended_at_unix_nanos: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetadataV1 {
    pub hostname: String,
    pub kernel_release: String,
    pub architecture: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRecordV1 {
    pub name: String,
    pub state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingV1 {
    pub id: String,
    pub severity: String,
    pub summary: String,
    pub evidence_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingEvidenceV1 {
    pub name: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipV1 {
    pub from: String,
    pub to: String,
    pub relationship: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RedactionRecordV1 {
    pub fields_removed: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAnalysisV1 {
    pub provider: String,
    pub model: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceBundleV1 {
    pub schema_version: u16,
    pub bundle_version: String,
    pub case_id: String,
    pub time_range: TimeRangeV1,
    pub node: NodeMetadataV1,
    pub capabilities: Vec<CapabilityRecordV1>,
    pub observations: Vec<Value>,
    pub findings: Vec<FindingV1>,
    pub missing_evidence: Vec<MissingEvidenceV1>,
    pub relationships: Vec<RelationshipV1>,
    pub alert_references: Vec<String>,
    pub redaction: RedactionRecordV1,
    pub ai_analysis: Option<AiAnalysisV1>,
}

impl EvidenceBundleV1 {
    pub fn validate(&self) -> AppResult<()> {
        if self.schema_version != EVIDENCE_SCHEMA_VERSION
            || self.bundle_version != "EvidenceBundleV1"
        {
            return Err(AppError::Validation(
                "unsupported evidence bundle version".into(),
            ));
        }
        if self.case_id.trim().is_empty()
            || self.time_range.ended_at_unix_nanos < self.time_range.started_at_unix_nanos
        {
            return Err(AppError::Validation(
                "evidence bundle has an invalid case ID or time range".into(),
            ));
        }
        Ok(())
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_nanos() as u64)
        .unwrap_or(0)
}
fn host_value(path: &str) -> String {
    fs::read_to_string(path)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}
fn capability_state(value: crate::forensics::CapabilityState) -> String {
    format!("{value:?}").to_lowercase()
}

pub fn capture(target: Option<&str>) -> AppResult<EvidenceBundleV1> {
    let started = now_ns();
    let caps = KernelCapabilities::detect_host();
    let capabilities: Vec<CapabilityRecordV1> = vec![
        ("btf", caps.btf),
        ("process_exit", caps.process_exit),
        ("signal_origin", caps.signal_origin),
        ("signal_delivery", caps.signal_delivery),
        ("oom_victim", caps.oom_victim),
    ]
    .into_iter()
    .map(|(name, state)| CapabilityRecordV1 {
        name: name.into(),
        state: capability_state(state),
    })
    .collect();
    let mut observations = vec![
        serde_json::json!({"id":"system_snapshot","kind":"system_snapshot","value":telemetry::collect_system_telemetry()}),
    ];
    let mut findings = Vec::new();
    let mut missing = Vec::new();
    let mut relationships = Vec::new();
    if let Some(target) = target {
        let pid = telemetry::find_pid_by_name(target);
        if pid == 0 {
            return Err(AppError::Validation(format!("process not found: {target}")));
        }
        let process = telemetry::collect_process_telemetry(pid);
        if process.name.is_empty() {
            missing.push(MissingEvidenceV1 {
                name: "process_snapshot".into(),
                reason: "process exited or became inaccessible during capture".into(),
            });
        }
        if process.state == "D" {
            findings.push(FindingV1 {
                id: "process_uninterruptible_sleep".into(),
                severity: "warning".into(),
                summary: format!("process {pid} is in uninterruptible sleep"),
                evidence_refs: vec!["process_snapshot".into()],
            });
        }
        if process.majflt > 0 {
            findings.push(FindingV1 {
                id: "major_page_faults_observed".into(),
                severity: "info".into(),
                summary: format!("process {pid} has {} major page faults", process.majflt),
                evidence_refs: vec!["process_snapshot".into()],
            });
        }
        if process.ppid > 0 {
            relationships.push(RelationshipV1 {
                from: format!("pid:{pid}"),
                to: format!("pid:{}", process.ppid),
                relationship: "spawned_by".into(),
            });
        }
        observations.push(
            serde_json::json!({"id":"process_snapshot","kind":"process_snapshot","value":process}),
        );
    }
    for cap in &capabilities {
        if cap.state != "supported" {
            missing.push(MissingEvidenceV1 {
                name: cap.name.clone(),
                reason: format!("capability is {}", cap.state),
            });
        }
    }
    let ended = now_ns();
    let case_id = format!(
        "{:x}-{}-{}",
        ended,
        std::process::id(),
        CASE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    Ok(EvidenceBundleV1 {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        bundle_version: "EvidenceBundleV1".into(),
        case_id,
        time_range: TimeRangeV1 {
            started_at_unix_nanos: started,
            ended_at_unix_nanos: ended,
        },
        node: NodeMetadataV1 {
            hostname: host_value("/etc/hostname"),
            kernel_release: host_value("/proc/sys/kernel/osrelease"),
            architecture: std::env::consts::ARCH.into(),
        },
        capabilities,
        observations,
        findings,
        missing_evidence: missing,
        relationships,
        alert_references: Vec::new(),
        redaction: RedactionRecordV1::default(),
        ai_analysis: None,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseSummary {
    pub case_id: String,
    pub captured_at_unix_nanos: u64,
    pub bytes: u64,
    pub findings: usize,
}

#[derive(Debug, Clone)]
pub struct CaseStore {
    root: PathBuf,
    retention_days: u64,
    retention_bytes: u64,
}

impl Default for CaseStore {
    fn default() -> Self {
        Self::new(
            config::get_syspilot_dir().join("cases"),
            DEFAULT_RETENTION_DAYS,
            DEFAULT_RETENTION_BYTES,
        )
    }
}

impl CaseStore {
    pub fn new(root: PathBuf, retention_days: u64, retention_bytes: u64) -> Self {
        Self {
            root,
            retention_days,
            retention_bytes,
        }
    }
    fn prepare(&self) -> AppResult<()> {
        fs::create_dir_all(&self.root)
            .map_err(|e| AppError::io("could not create case directory", e))?;
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
            .map_err(|e| AppError::io("could not secure case directory", e))?;
        for entry in fs::read_dir(&self.root)
            .map_err(|e| AppError::io("could not read case directory", e))?
        {
            let entry =
                entry.map_err(|e| AppError::io("could not inspect case directory entry", e))?;
            if entry.path().extension().and_then(|v| v.to_str()) == Some("tmp") {
                fs::remove_file(entry.path())
                    .map_err(|e| AppError::io("could not remove interrupted case file", e))?;
            }
        }
        Ok(())
    }
    fn path(&self, id: &str) -> AppResult<PathBuf> {
        if id.is_empty() || !id.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'-') {
            return Err(AppError::Validation("invalid case ID".into()));
        }
        Ok(self.root.join(format!("{id}.json")))
    }
    pub fn save(&self, bundle: &EvidenceBundleV1) -> AppResult<()> {
        bundle.validate()?;
        self.prepare()?;
        let path = self.path(&bundle.case_id)?;
        let temporary = self.root.join(format!("{}.tmp", bundle.case_id));
        let bytes = serde_json::to_vec_pretty(bundle)
            .map_err(|e| AppError::Protocol(format!("could not encode evidence bundle: {e}")))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| AppError::io("could not create temporary case", e))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| AppError::io("could not persist case", e))?;
        fs::rename(&temporary, &path).map_err(|e| AppError::io("could not commit case", e))?;
        self.enforce_retention()
    }
    pub fn load(&self, id: &str) -> AppResult<EvidenceBundleV1> {
        let path = self.path(id)?;
        let text = fs::read_to_string(path).map_err(|e| AppError::io("could not read case", e))?;
        let bundle: EvidenceBundleV1 = serde_json::from_str(&text)
            .map_err(|e| AppError::Protocol(format!("could not decode evidence bundle: {e}")))?;
        bundle.validate()?;
        Ok(bundle)
    }
    pub fn list(&self) -> AppResult<Vec<CaseSummary>> {
        self.prepare()?;
        let mut cases = Vec::new();
        for entry in fs::read_dir(&self.root)
            .map_err(|e| AppError::io("could not read case directory", e))?
        {
            let entry =
                entry.map_err(|e| AppError::io("could not inspect case directory entry", e))?;
            if entry.path().extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            let path = entry.path();
            let bytes =
                fs::read(&path).map_err(|e| AppError::io("could not read stored case", e))?;
            let bundle: EvidenceBundleV1 = serde_json::from_slice(&bytes).map_err(|e| {
                AppError::Protocol(format!(
                    "stored case {} is malformed and was not ignored: {e}",
                    path.display()
                ))
            })?;
            bundle.validate()?;
            let metadata = entry
                .metadata()
                .map_err(|e| AppError::io("could not read stored case metadata", e))?;
            cases.push(CaseSummary {
                case_id: bundle.case_id,
                captured_at_unix_nanos: bundle.time_range.ended_at_unix_nanos,
                bytes: metadata.len(),
                findings: bundle.findings.len(),
            });
        }
        cases.sort_by_key(|case| std::cmp::Reverse(case.captured_at_unix_nanos));
        Ok(cases)
    }
    pub fn export(&self, id: &str, destination: &Path) -> AppResult<()> {
        let bundle = self.load(id)?;
        let bytes =
            serde_json::to_vec_pretty(&bundle).map_err(|e| AppError::Protocol(e.to_string()))?;
        fs::write(destination, bytes).map_err(|e| AppError::io("could not export case", e))
    }
    pub fn delete(&self, id: &str) -> AppResult<()> {
        fs::remove_file(self.path(id)?).map_err(|e| AppError::io("could not delete case", e))
    }
    fn enforce_retention(&self) -> AppResult<()> {
        let cutoff =
            now_ns().saturating_sub(self.retention_days.saturating_mul(86_400_000_000_000));
        let mut cases = self.list()?;
        cases.sort_by_key(|v| v.captured_at_unix_nanos);
        let mut total: u64 = cases.iter().map(|v| v.bytes).sum();
        for case in cases {
            if case.captured_at_unix_nanos < cutoff || total > self.retention_bytes {
                let path = self.path(&case.case_id)?;
                fs::remove_file(path)
                    .map_err(|e| AppError::io("could not enforce case retention", e))?;
                total = total.saturating_sub(case.bytes);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(id: &str, timestamp: u64) -> EvidenceBundleV1 {
        EvidenceBundleV1 {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            bundle_version: "EvidenceBundleV1".into(),
            case_id: id.into(),
            time_range: TimeRangeV1 {
                started_at_unix_nanos: timestamp,
                ended_at_unix_nanos: timestamp,
            },
            node: NodeMetadataV1 {
                hostname: "node".into(),
                kernel_release: "kernel".into(),
                architecture: "x86_64".into(),
            },
            capabilities: Vec::new(),
            observations: vec![serde_json::json!({"id":"one"})],
            findings: Vec::new(),
            missing_evidence: Vec::new(),
            relationships: Vec::new(),
            alert_references: Vec::new(),
            redaction: RedactionRecordV1::default(),
            ai_analysis: None,
        }
    }

    #[test]
    fn atomic_case_round_trip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaseStore::new(dir.path().join("cases"), 30, u64::MAX);
        store.save(&bundle("case-1", now_ns())).unwrap();
        assert_eq!(store.load("case-1").unwrap().case_id, "case-1");
        assert_eq!(
            fs::metadata(store.path("case-1").unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn interrupted_temporary_case_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cases");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("interrupted.tmp"), b"partial").unwrap();
        let store = CaseStore::new(root.clone(), 30, u64::MAX);
        assert!(store.list().unwrap().is_empty());
        assert!(!root.join("interrupted.tmp").exists());
    }

    #[test]
    fn malformed_stored_case_is_reported_instead_of_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cases");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("corrupt.json"), b"{").unwrap();
        let store = CaseStore::new(root, 30, u64::MAX);
        let error = store.list().unwrap_err().to_string();
        assert!(error.contains("malformed"));
        assert!(error.contains("was not ignored"));
    }

    #[test]
    fn retention_evicts_cases_when_byte_limit_is_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaseStore::new(dir.path().join("cases"), u64::MAX / 86_400_000_000_000, 1);
        store.save(&bundle("old", 1)).unwrap();
        store.save(&bundle("new", 2)).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn rejects_path_traversal_case_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = CaseStore::new(dir.path().join("cases"), 30, u64::MAX);
        assert!(store.load("../secret").is_err());
    }
}
