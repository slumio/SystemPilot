//! Distributed telemetry contracts, delivery pipeline, and process-name alerting.
//!
//! The daemon only publishes typed envelopes. Transport, batching, retry behavior,
//! authentication, node identity, and alert rules are supplied by configuration.

use crate::error::{AppError, AppResult};
use crate::spool::{DiskSpool, DEFAULT_MAX_AGE_SECS, DEFAULT_MAX_BYTES};
use crossbeam_channel::{Receiver, Sender};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryKind {
    ProcessLifecycle,
    ProcessAlert,
    ProcessSnapshot,
    SystemSnapshot,
    CausalGraph,
    Health,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEnvelope {
    pub schema_version: u16,
    pub message_id: String,
    pub node_id: String,
    pub sequence: u64,
    pub observed_at_unix_nanos: u64,
    pub kind: TelemetryKind,
    pub payload: Value,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

impl TelemetryEnvelope {
    pub fn validate(&self) -> AppResult<()> {
        if self.schema_version != TELEMETRY_SCHEMA_VERSION {
            return Err(AppError::Protocol(format!(
                "unsupported telemetry schema version {}; expected {}",
                self.schema_version, TELEMETRY_SCHEMA_VERSION
            )));
        }
        if self.message_id.trim().is_empty() || self.node_id.trim().is_empty() {
            return Err(AppError::Validation(
                "telemetry message_id and node_id must not be empty".into(),
            ));
        }
        if self.observed_at_unix_nanos == 0 || self.payload.is_null() {
            return Err(AppError::Validation(
                "telemetry timestamp must be set and payload must not be null".into(),
            ));
        }
        Ok(())
    }

    pub fn encode_json(&self) -> AppResult<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| {
            AppError::Protocol(format!("could not encode telemetry envelope: {error}"))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RedactionPolicy {
    pub command_arguments: bool,
    pub environment_variables: bool,
    pub paths: bool,
    pub usernames: bool,
    pub ip_addresses: bool,
    pub source_snippets: bool,
}

impl Default for RedactionPolicy {
    fn default() -> Self {
        Self {
            command_arguments: true,
            environment_variables: true,
            paths: true,
            usernames: true,
            ip_addresses: true,
            source_snippets: true,
        }
    }
}

fn sensitive_key(key: &str, policy: &RedactionPolicy) -> bool {
    let key = key.to_ascii_lowercase();
    (policy.command_arguments
        && matches!(
            key.as_str(),
            "args" | "argv" | "command" | "command_line" | "cmdline"
        ))
        || (policy.environment_variables
            && matches!(key.as_str(), "env" | "environ" | "environment_variables"))
        || (policy.paths && (key == "path" || key.ends_with("_path") || key.ends_with("_paths")))
        || (policy.usernames && matches!(key.as_str(), "user" | "username" | "user_name" | "login"))
        || (policy.ip_addresses
            && (matches!(
                key.as_str(),
                "ip" | "address" | "remote_addr" | "local_addr"
            ) || key.ends_with("_ip")))
        || (policy.source_snippets
            && matches!(
                key.as_str(),
                "source" | "snippet" | "source_code" | "code_snippet"
            ))
}

/// Applies the configured privacy boundary recursively before queueing or
/// persistence. Object shape is retained so previews remain representative.
pub fn redact_value(value: &mut Value, policy: &RedactionPolicy) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if sensitive_key(key, policy) {
                    *child = Value::String("[REDACTED]".into());
                } else {
                    redact_value(child, policy);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_value(value, policy);
            }
        }
        Value::String(text) => {
            let contains_path = policy.paths
                && text.split_whitespace().any(|part| {
                    part.trim_matches(|character: char| {
                        matches!(character, ',' | ';' | '(' | ')' | '[' | ']')
                    })
                    .starts_with('/')
                });
            let contains_ip = policy.ip_addresses
                && text.split_whitespace().any(|part| {
                    let candidate = part.trim_matches(|character: char| {
                        matches!(character, ',' | ';' | '(' | ')' | '[' | ']')
                    });
                    candidate.parse::<std::net::IpAddr>().is_ok()
                        || candidate.rsplit_once(':').is_some_and(|(host, port)| {
                            port.parse::<u16>().is_ok() && host.parse::<std::net::IpAddr>().is_ok()
                        })
                });
            if contains_path || contains_ip {
                *text = "[REDACTED]".into();
            }
        }
        _ => {}
    }
}

pub fn preview_envelope<T: Serialize>(
    config: &DistributedTelemetryConfig,
    kind: TelemetryKind,
    payload: &T,
) -> AppResult<TelemetryEnvelope> {
    config.validate()?;
    let mut payload = serde_json::to_value(payload).map_err(|error| {
        AppError::Protocol(format!("could not serialize telemetry preview: {error}"))
    })?;
    redact_value(&mut payload, &config.redaction);
    let mut attributes_value = serde_json::to_value(&config.attributes).map_err(|error| {
        AppError::Protocol(format!("could not serialize telemetry attributes: {error}"))
    })?;
    redact_value(&mut attributes_value, &config.redaction);
    let attributes = serde_json::from_value(attributes_value).map_err(|error| {
        AppError::Protocol(format!(
            "could not decode redacted telemetry attributes: {error}"
        ))
    })?;
    let observed_at_unix_nanos = now_ns();
    Ok(TelemetryEnvelope {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        message_id: format!(
            "{}-preview",
            if config.node_id.is_empty() {
                "unenrolled-node"
            } else {
                &config.node_id
            }
        ),
        node_id: if config.node_id.is_empty() {
            "unenrolled-node".into()
        } else {
            config.node_id.clone()
        },
        sequence: 0,
        observed_at_unix_nanos,
        kind,
        payload,
        attributes,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPolicy {
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    pub max_queue_messages: usize,
    pub retry_backoff_ms: u64,
    pub max_retries: u32,
    pub request_timeout_ms: u64,
    pub spool_max_bytes: u64,
    pub spool_max_age_seconds: u64,
}

impl Default for ExportPolicy {
    fn default() -> Self {
        Self {
            batch_size: 256,
            flush_interval_ms: 1_000,
            max_queue_messages: 4_096,
            retry_backoff_ms: 1_000,
            max_retries: 3,
            request_timeout_ms: 10_000,
            spool_max_bytes: DEFAULT_MAX_BYTES,
            spool_max_age_seconds: DEFAULT_MAX_AGE_SECS,
        }
    }
}

impl ExportPolicy {
    pub fn validate(&self) -> AppResult<()> {
        if !(1..=10_000).contains(&self.batch_size)
            || self.flush_interval_ms == 0
            || self.max_queue_messages < self.batch_size
            || self.request_timeout_ms == 0
            || self.spool_max_bytes == 0
            || self.spool_max_age_seconds == 0
        {
            return Err(AppError::Validation("distributed export policy has an invalid batch, queue, flush interval, or request timeout".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessNameMatch {
    Exact,
    Prefix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessAlertRule {
    pub id: String,
    pub process_name: String,
    #[serde(default = "default_process_name_match")]
    pub match_type: ProcessNameMatch,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

fn default_process_name_match() -> ProcessNameMatch {
    ProcessNameMatch::Exact
}

impl ProcessAlertRule {
    pub fn validate(&self) -> AppResult<()> {
        if self.id.trim().is_empty() || self.process_name.trim().is_empty() {
            return Err(AppError::Validation(
                "process alert rule id and process_name must not be empty".into(),
            ));
        }
        Ok(())
    }

    pub fn matches(&self, process_name: &str) -> bool {
        self.enabled
            && match self.match_type {
                ProcessNameMatch::Exact => process_name == self.process_name,
                ProcessNameMatch::Prefix => process_name.starts_with(&self.process_name),
            }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessAlert {
    #[serde(default)]
    pub instance_id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub previous_state: Option<String>,
    pub rule_id: String,
    pub process_name: String,
    pub pid: i32,
    pub ppid: i32,
    pub event_type: String,
    pub observed_at_unix_nanos: u64,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessAlertEngine {
    rules: Vec<ProcessAlertRule>,
}

impl ProcessAlertEngine {
    pub fn new(rules: Vec<ProcessAlertRule>) -> AppResult<Self> {
        let mut ids = std::collections::BTreeSet::new();
        for rule in &rules {
            rule.validate()?;
            if !ids.insert(rule.id.clone()) {
                return Err(AppError::Validation(format!(
                    "duplicate process alert rule id: {}",
                    rule.id
                )));
            }
        }
        Ok(Self { rules })
    }

    pub fn evaluate(
        &self,
        process_name: &str,
        pid: i32,
        ppid: i32,
        event_type: &str,
        observed_at_unix_nanos: u64,
    ) -> Vec<ProcessAlert> {
        self.rules
            .iter()
            .filter(|rule| rule.matches(process_name))
            .map(|rule| ProcessAlert {
                rule_id: rule.id.clone(),
                instance_id: String::new(),
                state: String::new(),
                previous_state: None,
                process_name: process_name.to_string(),
                pid,
                ppid,
                event_type: event_type.to_string(),
                observed_at_unix_nanos,
                labels: rule.labels.clone(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct DistributedTelemetryConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub node_id: String,
    pub bearer_token: String,
    pub attributes: BTreeMap<String, String>,
    pub export_policy: ExportPolicy,
    pub process_alert_rules: Vec<ProcessAlertRule>,
    pub redaction: RedactionPolicy,
}

impl DistributedTelemetryConfig {
    pub fn validate(&self) -> AppResult<()> {
        self.export_policy.validate()?;
        ProcessAlertEngine::new(self.process_alert_rules.clone())?;
        if self.enabled {
            if self.endpoint.trim().is_empty() || self.node_id.trim().is_empty() {
                return Err(AppError::Validation(
                    "distributed telemetry requires endpoint and node_id when enabled".into(),
                ));
            }
            reqwest::Url::parse(&self.endpoint).map_err(|error| {
                AppError::Validation(format!("invalid distributed telemetry endpoint: {error}"))
            })?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IngestionAcknowledgement {
    pub accepted_message_ids: Vec<String>,
    pub highest_accepted_sequence: Option<u64>,
    pub rejected_records: Vec<RejectedRecord>,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedRecord {
    pub message_id: String,
    pub reason: String,
}

impl IngestionAcknowledgement {
    fn whole_batch(batch: &[TelemetryEnvelope]) -> Self {
        Self {
            accepted_message_ids: batch
                .iter()
                .map(|record| record.message_id.clone())
                .collect(),
            highest_accepted_sequence: batch.iter().map(|record| record.sequence).max(),
            rejected_records: Vec::new(),
            retry_after_ms: None,
        }
    }

    fn validate(&self, batch: &[TelemetryEnvelope]) -> AppResult<()> {
        let submitted: std::collections::BTreeSet<&str> = batch
            .iter()
            .map(|record| record.message_id.as_str())
            .collect();
        let mut decided = std::collections::BTreeSet::new();
        for id in &self.accepted_message_ids {
            if !submitted.contains(id.as_str()) || !decided.insert(id.as_str()) {
                return Err(AppError::Protocol(format!("collector acknowledgement contains unknown or duplicate accepted message ID: {id}")));
            }
        }
        for rejected in &self.rejected_records {
            if rejected.reason.trim().is_empty()
                || !submitted.contains(rejected.message_id.as_str())
                || !decided.insert(rejected.message_id.as_str())
            {
                return Err(AppError::Protocol(format!(
                    "collector acknowledgement contains invalid rejected record: {}",
                    rejected.message_id
                )));
            }
        }
        if let Some(highest) = self.highest_accepted_sequence {
            let actual = batch
                .iter()
                .filter(|record| self.accepted_message_ids.contains(&record.message_id))
                .map(|record| record.sequence)
                .max();
            if actual != Some(highest) {
                return Err(AppError::Protocol(
                    "collector highest accepted sequence does not match accepted message IDs"
                        .into(),
                ));
            }
        }
        if self.retry_after_ms.is_some_and(|delay| delay > 86_400_000) {
            return Err(AppError::Protocol(
                "collector retry_after_ms exceeds the one-day safety bound".into(),
            ));
        }
        if decided.is_empty() {
            return Err(AppError::Protocol(
                "collector acknowledgement made no decision for the submitted batch".into(),
            ));
        }
        Ok(())
    }
}

pub trait TelemetrySink: Send + Sync + 'static {
    fn export(&self, batch: &[TelemetryEnvelope]) -> AppResult<IngestionAcknowledgement>;
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ExporterHealth {
    pub queued: u64,
    pub sent: u64,
    pub retried: u64,
    pub rejected: u64,
    pub dropped: u64,
    pub last_acknowledgement_unix_nanos: Option<u64>,
    pub spool_bytes: u64,
    pub quarantined: u64,
    pub persistence_failures: u64,
}

#[derive(Default)]
struct ExportMetrics {
    queued: AtomicU64,
    sent: AtomicU64,
    retried: AtomicU64,
    rejected: AtomicU64,
    dropped: AtomicU64,
    last_acknowledgement_unix_nanos: AtomicU64,
    spool_bytes: AtomicU64,
    quarantined: AtomicU64,
    persistence_failures: AtomicU64,
}

impl ExportMetrics {
    fn snapshot(&self) -> ExporterHealth {
        let last = self.last_acknowledgement_unix_nanos.load(Ordering::Relaxed);
        ExporterHealth {
            queued: self.queued.load(Ordering::Relaxed),
            sent: self.sent.load(Ordering::Relaxed),
            retried: self.retried.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            last_acknowledgement_unix_nanos: (last != 0).then_some(last),
            spool_bytes: self.spool_bytes.load(Ordering::Relaxed),
            quarantined: self.quarantined.load(Ordering::Relaxed),
            persistence_failures: self.persistence_failures.load(Ordering::Relaxed),
        }
    }
}

pub struct HttpTelemetrySink {
    client: Client,
    endpoint: String,
    bearer_token: String,
}

impl HttpTelemetrySink {
    pub fn new(config: &DistributedTelemetryConfig) -> AppResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(
                config.export_policy.request_timeout_ms,
            ))
            .build()
            .map_err(|error| {
                AppError::Protocol(format!("could not build telemetry HTTP client: {error}"))
            })?;
        Ok(Self {
            client,
            endpoint: config.endpoint.clone(),
            bearer_token: config.bearer_token.clone(),
        })
    }
}

impl TelemetrySink for HttpTelemetrySink {
    fn export(&self, batch: &[TelemetryEnvelope]) -> AppResult<IngestionAcknowledgement> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .header(CONTENT_TYPE, "application/json")
            .json(batch);
        if !self.bearer_token.is_empty() {
            request = request.header(AUTHORIZATION, format!("Bearer {}", self.bearer_token));
        }
        let response = request
            .send()
            .map_err(|error| {
                AppError::Protocol(format!("telemetry export request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| AppError::Protocol(format!("telemetry export rejected: {error}")))?;
        let bytes = response.bytes().map_err(|error| {
            AppError::Protocol(format!("could not read collector acknowledgement: {error}"))
        })?;
        if bytes.is_empty() {
            return Ok(IngestionAcknowledgement::whole_batch(batch));
        }
        let acknowledgement: IngestionAcknowledgement =
            serde_json::from_slice(&bytes).map_err(|error| {
                AppError::Protocol(format!("invalid collector acknowledgement: {error}"))
            })?;
        acknowledgement.validate(batch)?;
        Ok(acknowledgement)
    }
}

pub struct TelemetryPublisher {
    sender: Option<Sender<TelemetryEnvelope>>,
    node_id: String,
    attributes: BTreeMap<String, String>,
    metrics: Arc<ExportMetrics>,
    spool: Option<Arc<DiskSpool>>,
    redaction: RedactionPolicy,
}

impl TelemetryPublisher {
    pub fn disabled() -> Self {
        Self {
            sender: None,
            node_id: String::new(),
            attributes: BTreeMap::new(),
            metrics: Arc::new(ExportMetrics::default()),
            spool: None,
            redaction: RedactionPolicy::default(),
        }
    }

    pub fn from_config(config: &DistributedTelemetryConfig) -> AppResult<Self> {
        config.validate()?;
        if !config.enabled {
            return Ok(Self::disabled());
        }
        let sink = Arc::new(HttpTelemetrySink::new(config)?);
        let (sender, receiver) =
            crossbeam_channel::bounded(config.export_policy.max_queue_messages);
        let spool = Arc::new(DiskSpool::open(
            crate::config::get_syspilot_dir().join("telemetry-spool"),
            config.export_policy.spool_max_bytes,
            config.export_policy.spool_max_age_seconds,
        )?);
        let stats = spool.stats()?;
        let metrics = Arc::new(ExportMetrics::default());
        metrics.queued.store(stats.records, Ordering::Relaxed);
        metrics.spool_bytes.store(stats.bytes, Ordering::Relaxed);
        metrics
            .quarantined
            .store(stats.quarantined, Ordering::Relaxed);
        spawn_export_worker(
            receiver,
            sink,
            config.export_policy.clone(),
            Arc::clone(&metrics),
            Arc::clone(&spool),
        );
        Ok(Self {
            sender: Some(sender),
            node_id: config.node_id.clone(),
            attributes: config.attributes.clone(),
            metrics,
            redaction: config.redaction.clone(),
            spool: Some(spool),
        })
    }

    pub fn publish<T: Serialize>(&self, kind: TelemetryKind, payload: &T) -> AppResult<()> {
        let Some(sender) = &self.sender else {
            return Ok(());
        };
        let spool = self
            .spool
            .as_ref()
            .ok_or_else(|| AppError::Protocol("enabled telemetry publisher has no spool".into()))?;
        let mut payload = serde_json::to_value(payload).map_err(|error| {
            AppError::Protocol(format!("could not serialize telemetry payload: {error}"))
        })?;
        redact_value(&mut payload, &self.redaction);
        let sequence = spool.next_sequence().inspect_err(|_| {
            self.metrics
                .persistence_failures
                .fetch_add(1, Ordering::Relaxed);
        })?;
        let observed_at_unix_nanos = now_ns();
        let envelope = TelemetryEnvelope {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            message_id: format!("{}-{}-{}", self.node_id, observed_at_unix_nanos, sequence),
            node_id: self.node_id.clone(),
            sequence,
            observed_at_unix_nanos,
            kind,
            payload,
            attributes: self.attributes.clone(),
        };
        spool.append(&envelope).inspect_err(|_| {
            self.metrics
                .persistence_failures
                .fetch_add(1, Ordering::Relaxed);
        })?;
        let stats = spool.stats()?;
        self.metrics.queued.store(stats.records, Ordering::Relaxed);
        self.metrics
            .spool_bytes
            .store(stats.bytes, Ordering::Relaxed);
        self.metrics
            .quarantined
            .store(stats.quarantined, Ordering::Relaxed);
        // This is only a wake-up accelerator; the durable record is authoritative.
        let _ = sender.try_send(envelope);
        Ok(())
    }
    pub fn dropped_messages(&self) -> u64 {
        self.metrics.dropped.load(Ordering::Relaxed)
    }

    pub fn health(&self) -> ExporterHealth {
        self.metrics.snapshot()
    }
}

fn spawn_export_worker(
    receiver: Receiver<TelemetryEnvelope>,
    sink: Arc<dyn TelemetrySink>,
    policy: ExportPolicy,
    metrics: Arc<ExportMetrics>,
    spool: Arc<DiskSpool>,
) {
    std::thread::spawn(move || loop {
        match receiver.recv_timeout(Duration::from_millis(policy.flush_interval_ms)) {
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }
        while receiver.try_recv().is_ok() {}
        let batch = match spool.pending(policy.batch_size) {
            Ok(batch) => batch,
            Err(error) => {
                metrics.persistence_failures.fetch_add(1, Ordering::Relaxed);
                tracing::error!("[telemetry] could not recover durable spool: {error}");
                continue;
            }
        };
        if batch.is_empty() {
            continue;
        }
        let mut delivered = false;
        for attempt in 0..=policy.max_retries {
            if attempt > 0 {
                metrics
                    .retried
                    .fetch_add(batch.len() as u64, Ordering::Relaxed);
            }
            match sink.export(&batch) {
                Ok(acknowledgement) => {
                    if let Err(error) = acknowledgement.validate(&batch) {
                        tracing::warn!("[telemetry] invalid collector acknowledgement; durable batch retained: {error}");
                        continue;
                    }
                    let accepted: Vec<TelemetryEnvelope> = batch
                        .iter()
                        .filter(|record| {
                            acknowledgement
                                .accepted_message_ids
                                .contains(&record.message_id)
                        })
                        .cloned()
                        .collect();
                    if !accepted.is_empty() {
                        if let Err(error) = spool.acknowledge(&accepted) {
                            metrics.persistence_failures.fetch_add(1, Ordering::Relaxed);
                            tracing::error!("[telemetry] collector acknowledged records but spool commit failed: {error}");
                            break;
                        }
                        metrics
                            .queued
                            .fetch_sub(accepted.len() as u64, Ordering::Relaxed);
                        metrics
                            .sent
                            .fetch_add(accepted.len() as u64, Ordering::Relaxed);
                        metrics
                            .last_acknowledgement_unix_nanos
                            .store(now_ns(), Ordering::Relaxed);
                    }
                    if !acknowledgement.rejected_records.is_empty() {
                        metrics.rejected.fetch_add(
                            acknowledgement.rejected_records.len() as u64,
                            Ordering::Relaxed,
                        );
                        for rejected in &acknowledgement.rejected_records {
                            tracing::error!(
                                "[telemetry] collector rejected {}: {} (record retained)",
                                rejected.message_id,
                                rejected.reason
                            );
                        }
                    }
                    if let Some(delay) = acknowledgement.retry_after_ms {
                        std::thread::sleep(Duration::from_millis(delay));
                    }
                    delivered = true;
                    break;
                }
                Err(error) => {
                    tracing::warn!(
                        "[telemetry] export attempt {} failed; durable batch retained: {}",
                        attempt + 1,
                        error
                    );
                    if attempt < policy.max_retries {
                        let exponent = attempt.min(16);
                        let base = policy.retry_backoff_ms.saturating_mul(1_u64 << exponent);
                        let jitter = now_ns() % (base / 4 + 1);
                        std::thread::sleep(Duration::from_millis(base.saturating_add(jitter)));
                    }
                }
            }
        }
        if !delivered {
            metrics
                .rejected
                .fetch_add(batch.len() as u64, Ordering::Relaxed);
            tracing::error!(
                "[telemetry] retry budget exhausted; durable batch retained for replay"
            );
        }
        match spool.stats() {
            Ok(stats) => {
                metrics.queued.store(stats.records, Ordering::Relaxed);
                metrics.spool_bytes.store(stats.bytes, Ordering::Relaxed);
                metrics
                    .quarantined
                    .store(stats.quarantined, Ordering::Relaxed);
            }
            Err(error) => {
                metrics.persistence_failures.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    "[telemetry] could not inspect durable spool after delivery: {error}"
                );
            }
        }
    });
}
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod delivery_tests {
    use super::*;

    struct TestSink {
        fail: bool,
    }
    impl TelemetrySink for TestSink {
        fn export(&self, batch: &[TelemetryEnvelope]) -> AppResult<IngestionAcknowledgement> {
            if self.fail {
                Err(AppError::Protocol("rejected".into()))
            } else {
                Ok(IngestionAcknowledgement::whole_batch(batch))
            }
        }
    }

    fn publisher(sink: Arc<dyn TelemetrySink>, retries: u32) -> TelemetryPublisher {
        let (sender, receiver) = crossbeam_channel::bounded(4);
        let metrics = Arc::new(ExportMetrics::default());
        let root = tempfile::tempdir().expect("temporary spool").keep();
        let spool = Arc::new(DiskSpool::open(root, 1024 * 1024, 60).expect("test spool"));
        spawn_export_worker(
            receiver,
            sink,
            ExportPolicy {
                batch_size: 1,
                flush_interval_ms: 1,
                max_queue_messages: 4,
                retry_backoff_ms: 1,
                max_retries: retries,
                request_timeout_ms: 10,
                spool_max_bytes: 1024 * 1024,
                spool_max_age_seconds: 60,
            },
            Arc::clone(&metrics),
            Arc::clone(&spool),
        );
        TelemetryPublisher {
            sender: Some(sender),
            node_id: "node".into(),
            attributes: BTreeMap::new(),
            metrics,
            spool: Some(spool),
            redaction: RedactionPolicy::default(),
        }
    }
    fn wait_for(mut predicate: impl FnMut() -> bool) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("export worker did not reach expected state");
    }

    #[test]
    fn successful_delivery_records_acknowledgement() {
        let publisher = publisher(Arc::new(TestSink { fail: false }), 0);
        publisher
            .publish(TelemetryKind::Health, &serde_json::json!({"ok":true}))
            .unwrap();
        wait_for(|| publisher.health().sent == 1);
        let health = publisher.health();
        assert_eq!(health.queued, 0);
        assert_eq!(health.sent, 1);
        assert_eq!(health.retried, 0);
        assert_eq!(health.rejected, 0);
        assert!(health.last_acknowledgement_unix_nanos.is_some());
    }

    #[test]
    fn exhausted_delivery_records_retry_and_rejection() {
        let publisher = publisher(Arc::new(TestSink { fail: true }), 1);
        publisher
            .publish(TelemetryKind::Health, &serde_json::json!({"ok":true}))
            .unwrap();
        wait_for(|| publisher.health().rejected == 1);
        let health = publisher.health();
        assert_eq!(health.queued, 1);
        assert_eq!(health.sent, 0);
        assert_eq!(health.retried, 1);
        assert_eq!(health.rejected, 1);
        assert!(health.last_acknowledgement_unix_nanos.is_none());
    }
}

#[cfg(test)]
mod acknowledgement_tests {
    use super::*;

    fn batch() -> Vec<TelemetryEnvelope> {
        vec![TelemetryEnvelope {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            message_id: "message-1".into(),
            node_id: "node".into(),
            sequence: 1,
            observed_at_unix_nanos: 1,
            kind: TelemetryKind::Health,
            payload: serde_json::json!({"ok": true}),
            attributes: BTreeMap::new(),
        }]
    }

    #[test]
    fn acknowledgement_rejects_unknown_message_id() {
        let acknowledgement = IngestionAcknowledgement {
            accepted_message_ids: vec!["unknown".into()],
            highest_accepted_sequence: Some(1),
            ..Default::default()
        };
        assert!(acknowledgement.validate(&batch()).is_err());
    }

    #[test]
    fn acknowledgement_requires_consistent_highest_sequence() {
        let acknowledgement = IngestionAcknowledgement {
            accepted_message_ids: vec!["message-1".into()],
            highest_accepted_sequence: Some(2),
            ..Default::default()
        };
        assert!(acknowledgement.validate(&batch()).is_err());
    }
}
