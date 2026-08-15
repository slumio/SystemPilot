use crate::config::Config;
use crate::credentials::CredentialRef;
use crate::distributed::ExporterHealth;
use crate::error::{AppError, AppResult};
use crate::output::{CapabilityState, DiagnosticV1, Outcome, OutputEnvelopeV1};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FleetEnrollment {
    pub enabled: bool,
    pub endpoint: String,
    pub node_id: String,
    #[serde(skip)]
    pub credential: String,
    pub credential_ref: CredentialRef,
    pub enrolled_at_unix_nanos: u64,
    pub policy_source: String,
    pub upload_scope: Vec<String>,
    pub last_acknowledgement_unix_nanos: Option<u64>,
}

impl FleetEnrollment {
    pub fn validate(&self) -> AppResult<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.node_id.trim().is_empty() || self.credential.trim().is_empty() {
            return Err(AppError::Validation(
                "fleet enrollment requires a node ID and credential".into(),
            ));
        }
        let endpoint = reqwest::Url::parse(&self.endpoint)
            .map_err(|error| AppError::Validation(format!("invalid fleet endpoint: {error}")))?;
        if endpoint.scheme() != "https" {
            return Err(AppError::Validation(
                "hosted fleet enrollment requires an HTTPS endpoint".into(),
            ));
        }
        Ok(())
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0)
}

pub fn enroll(
    config: &mut Config,
    endpoint: String,
    node_id: String,
    credential: String,
) -> AppResult<()> {
    let candidate = FleetEnrollment {
        enabled: true,
        endpoint: endpoint.clone(),
        node_id: node_id.clone(),
        credential: credential.clone(),
        credential_ref: CredentialRef::None,
        enrolled_at_unix_nanos: now_ns(),
        policy_source: "local_enrollment".into(),
        upload_scope: vec![
            "process_lifecycle".into(),
            "process_alert".into(),
            "health".into(),
        ],
        last_acknowledgement_unix_nanos: None,
    };
    candidate.validate()?;
    let credential_ref = crate::credentials::store_owner_secret(
        &crate::config::get_syspilot_dir().join("credentials"),
        "fleet-token",
        &credential,
    )?;
    enroll_with_reference(config, endpoint, node_id, credential, credential_ref)
}

pub fn enroll_with_reference(
    config: &mut Config,
    endpoint: String,
    node_id: String,
    credential: String,
    credential_ref: CredentialRef,
) -> AppResult<()> {
    let enrollment = FleetEnrollment {
        enabled: true,
        endpoint: endpoint.clone(),
        node_id: node_id.clone(),
        credential: credential.clone(),
        credential_ref: credential_ref.clone(),
        enrolled_at_unix_nanos: now_ns(),
        policy_source: "local_enrollment".into(),
        upload_scope: vec![
            "process_lifecycle".into(),
            "process_alert".into(),
            "health".into(),
        ],
        last_acknowledgement_unix_nanos: None,
    };
    enrollment.validate()?;
    config.fleet = enrollment;
    config.distributed_telemetry.enabled = true;
    config.distributed_telemetry.endpoint = endpoint;
    config.distributed_telemetry.node_id = node_id;
    config.distributed_telemetry.bearer_token = credential;
    config.distributed_telemetry.bearer_credential = credential_ref;
    crate::config::validate(config)
}

pub fn disable(config: &mut Config) {
    let owns_destination = config.fleet.enabled
        && config.distributed_telemetry.endpoint == config.fleet.endpoint
        && config.distributed_telemetry.node_id == config.fleet.node_id;
    if owns_destination {
        config.distributed_telemetry.enabled = false;
        config.distributed_telemetry.bearer_token.clear();
        config.distributed_telemetry.bearer_credential = CredentialRef::None;
    }
    config.fleet.enabled = false;
    config.fleet.credential.clear();
    config.fleet.credential_ref = CredentialRef::None;
}

#[derive(Debug, Clone, Serialize)]
pub struct FleetStatusV1 {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub node_id: Option<String>,
    pub policy_source: Option<String>,
    pub upload_scope: Vec<String>,
    pub credential_configured: bool,
    pub credential_source: String,
    pub delivery: Option<ExporterHealth>,
    pub last_acknowledgement_unix_nanos: Option<u64>,
}

pub fn collect_status(config: &Config) -> AppResult<OutputEnvelopeV1<FleetStatusV1>> {
    let mut diagnostics = Vec::new();
    let mut delivery = None;
    if config.fleet.enabled {
        match crate::daemon_health::read(&crate::config::daemon_health_path()) {
            Ok(snapshot) => delivery = Some(snapshot.health.exporter),
            Err(error) => diagnostics.push(DiagnosticV1 {
                component: "fleet_delivery".into(),
                state: if error.to_string().contains("malformed") {
                    CapabilityState::Misconfigured
                } else {
                    CapabilityState::Unavailable
                },
                detail: error.to_string(),
                impact: "delivery progress is unavailable; local diagnostics remain available"
                    .into(),
                recovery_command: Some("syspilot daemon".into()),
                fallback_used: None,
            }),
        }
    }
    let last_acknowledgement_unix_nanos = delivery
        .and_then(|value| value.last_acknowledgement_unix_nanos)
        .or(config.fleet.last_acknowledgement_unix_nanos);
    let outcome = if diagnostics
        .iter()
        .any(|item| item.state == CapabilityState::Misconfigured)
    {
        Outcome::Error
    } else if diagnostics.is_empty() {
        Outcome::Ok
    } else {
        Outcome::Degraded
    };
    OutputEnvelopeV1::new(
        "fleet.status",
        outcome,
        FleetStatusV1 {
            enabled: config.fleet.enabled,
            endpoint: config.fleet.enabled.then(|| config.fleet.endpoint.clone()),
            node_id: config.fleet.enabled.then(|| config.fleet.node_id.clone()),
            policy_source: config
                .fleet
                .enabled
                .then(|| config.fleet.policy_source.clone()),
            upload_scope: config.fleet.upload_scope.clone(),
            credential_configured: !config.fleet.credential.is_empty(),
            credential_source: crate::credentials::CredentialResolver::status(
                &config.fleet.credential_ref,
            )
            .source
            .into(),
            delivery,
            last_acknowledgement_unix_nanos,
        },
        diagnostics,
    )
}

pub fn print_status(config: &Config) {
    if !config.fleet.enabled {
        println!("Fleet enrollment: disabled");
        println!("Local diagnostics remain available.");
        return;
    }
    println!("Fleet enrollment: enabled");
    println!("Endpoint: {}", config.fleet.endpoint);
    println!("Node ID: {}", config.fleet.node_id);
    println!("Policy source: {}", config.fleet.policy_source);
    println!("Upload scope: {}", config.fleet.upload_scope.join(", "));
    println!("Credential: configured (not displayed)");
    let exporter = match crate::daemon_health::read(&crate::config::daemon_health_path()) {
        Ok(snapshot) => Some(snapshot.health.exporter),
        Err(error) => {
            println!("Delivery: unavailable: {error}. Recovery: run `syspilot daemon`.");
            None
        }
    };
    if let Some(exporter) = &exporter {
        println!(
            "Delivery: queued={} sent={} retried={} rejected={} dropped={} spool_bytes={} quarantined={} persistence_failures={}",
            exporter.queued, exporter.sent, exporter.retried, exporter.rejected,
            exporter.dropped, exporter.spool_bytes, exporter.quarantined,
            exporter.persistence_failures,
        );
    }
    let last_ack = exporter
        .and_then(|value| value.last_acknowledgement_unix_nanos)
        .or(config.fleet.last_acknowledgement_unix_nanos);
    match last_ack {
        Some(value) => println!("Last acknowledgement: {value} ns since Unix epoch"),
        None => println!("Last acknowledgement: none observed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_home(test: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("SYSPILOT_HOME");
        std::env::set_var("SYSPILOT_HOME", directory.path());
        test();
        match previous {
            Some(value) => std::env::set_var("SYSPILOT_HOME", value),
            None => std::env::remove_var("SYSPILOT_HOME"),
        }
    }
    #[test]
    fn enrollment_requires_https() {
        with_home(|| {
            let mut cfg = Config::default();
            assert!(enroll(
                &mut cfg,
                "http://fleet.example/ingest".into(),
                "node".into(),
                "token".into()
            )
            .is_err());
        });
    }
    #[test]
    fn enrollment_uses_public_export_destination_and_disable_clears_secrets() {
        with_home(|| {
            let mut cfg = Config::default();
            enroll(
                &mut cfg,
                "https://fleet.example/ingest".into(),
                "node-a".into(),
                "secret".into(),
            )
            .unwrap();
            assert!(cfg.distributed_telemetry.enabled);
            assert_eq!(cfg.distributed_telemetry.endpoint, cfg.fleet.endpoint);
            assert!(!serde_json::to_string(&cfg).unwrap().contains("secret"));
            disable(&mut cfg);
            assert!(!cfg.fleet.enabled);
            assert!(!cfg.distributed_telemetry.enabled);
            assert!(cfg.fleet.credential.is_empty());
            assert!(cfg.distributed_telemetry.bearer_token.is_empty());
        });
    }
}
