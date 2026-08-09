//! Distributed telemetry contracts, delivery pipeline, and process-name alerting.
//!
//! The daemon only publishes typed envelopes. Transport, batching, retry behavior,
//! authentication, node identity, and alert rules are supplied by configuration.

use crate::error::{AppError, AppResult};
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
pub struct ExportPolicy {
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    pub max_queue_messages: usize,
    pub retry_backoff_ms: u64,
    pub max_retries: u32,
    pub request_timeout_ms: u64,
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
        }
    }
}

impl ExportPolicy {
    pub fn validate(&self) -> AppResult<()> {
        if !(1..=10_000).contains(&self.batch_size)
            || self.flush_interval_ms == 0
            || self.max_queue_messages < self.batch_size
            || self.request_timeout_ms == 0
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
pub struct DistributedTelemetryConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub node_id: String,
    pub bearer_token: String,
    pub attributes: BTreeMap<String, String>,
    pub export_policy: ExportPolicy,
    pub process_alert_rules: Vec<ProcessAlertRule>,
}

impl Default for DistributedTelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            node_id: String::new(),
            bearer_token: String::new(),
            attributes: BTreeMap::new(),
            export_policy: ExportPolicy::default(),
            process_alert_rules: Vec::new(),
        }
    }
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

pub trait TelemetrySink: Send + Sync + 'static {
    fn export(&self, batch: &[TelemetryEnvelope]) -> AppResult<()>;
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
    fn export(&self, batch: &[TelemetryEnvelope]) -> AppResult<()> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .header(CONTENT_TYPE, "application/json")
            .json(batch);
        if !self.bearer_token.is_empty() {
            request = request.header(AUTHORIZATION, format!("Bearer {}", self.bearer_token));
        }
        request
            .send()
            .map_err(|error| {
                AppError::Protocol(format!("telemetry export request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| AppError::Protocol(format!("telemetry export rejected: {error}")))?;
        Ok(())
    }
}

pub struct TelemetryPublisher {
    sender: Option<Sender<TelemetryEnvelope>>,
    node_id: String,
    attributes: BTreeMap<String, String>,
    sequence: AtomicU64,
    dropped: AtomicU64,
}

impl TelemetryPublisher {
    pub fn disabled() -> Self {
        Self {
            sender: None,
            node_id: String::new(),
            attributes: BTreeMap::new(),
            sequence: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
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
        spawn_export_worker(receiver, sink, config.export_policy.clone());
        Ok(Self {
            sender: Some(sender),
            node_id: config.node_id.clone(),
            attributes: config.attributes.clone(),
            sequence: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        })
    }

    pub fn publish<T: Serialize>(&self, kind: TelemetryKind, payload: &T) {
        let Some(sender) = &self.sender else {
            return;
        };
        let payload = match serde_json::to_value(payload) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!("[telemetry] could not serialize payload: {error}");
                return;
            }
        };
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
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
        if sender.try_send(envelope).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped_messages(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

fn spawn_export_worker(
    receiver: Receiver<TelemetryEnvelope>,
    sink: Arc<dyn TelemetrySink>,
    policy: ExportPolicy,
) {
    std::thread::spawn(move || loop {
        let first = match receiver.recv_timeout(Duration::from_millis(policy.flush_interval_ms)) {
            Ok(item) => item,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        };
        let mut batch = Vec::with_capacity(policy.batch_size);
        batch.push(first);
        while batch.len() < policy.batch_size {
            match receiver.try_recv() {
                Ok(item) => batch.push(item),
                Err(_) => break,
            }
        }
        let mut delivered = false;
        for attempt in 0..=policy.max_retries {
            match sink.export(&batch) {
                Ok(()) => {
                    delivered = true;
                    break;
                }
                Err(error) => {
                    tracing::warn!(
                        "[telemetry] export attempt {} failed: {}",
                        attempt + 1,
                        error
                    );
                    if attempt < policy.max_retries {
                        std::thread::sleep(Duration::from_millis(policy.retry_backoff_ms));
                    }
                }
            }
        }
        if !delivered {
            tracing::error!("[telemetry] dropping batch after retry budget exhausted");
        }
    });
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}
