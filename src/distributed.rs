//! Versioned, transport-neutral primitives for future distributed telemetry.
//!
//! This module deliberately does not open network connections yet. It defines
//! the stable envelope and validation rules that an agent, collector, queue,
//! and query service can share when distributed export is introduced.

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryKind {
    ProcessLifecycle,
    ProcessSnapshot,
    SystemSnapshot,
    CausalGraph,
    Health,
}

/// A transport-neutral message. `sequence` is monotonic per node, enabling a
/// collector to deduplicate retries and detect gaps without relying on clocks.
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
        if self.message_id.trim().is_empty() {
            return Err(AppError::Validation(
                "telemetry message_id must not be empty".into(),
            ));
        }
        if self.node_id.trim().is_empty() {
            return Err(AppError::Validation(
                "telemetry node_id must not be empty".into(),
            ));
        }
        if self.observed_at_unix_nanos == 0 {
            return Err(AppError::Validation(
                "telemetry observed_at_unix_nanos must be set".into(),
            ));
        }
        if self.payload.is_null() {
            return Err(AppError::Validation(
                "telemetry payload must not be null".into(),
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
pub struct ExportPolicy {
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    pub max_queue_bytes: usize,
    pub retry_backoff_ms: u64,
}

impl Default for ExportPolicy {
    fn default() -> Self {
        Self {
            batch_size: 256,
            flush_interval_ms: 1_000,
            max_queue_bytes: 64 * 1024 * 1024,
            retry_backoff_ms: 1_000,
        }
    }
}

impl ExportPolicy {
    pub fn validate(&self) -> AppResult<()> {
        if self.batch_size == 0 || self.batch_size > 10_000 {
            return Err(AppError::Validation(
                "distributed export batch_size must be between 1 and 10000".into(),
            ));
        }
        if self.flush_interval_ms == 0 {
            return Err(AppError::Validation(
                "distributed export flush_interval_ms must be greater than zero".into(),
            ));
        }
        if self.max_queue_bytes < 1024 * 1024 {
            return Err(AppError::Validation(
                "distributed export max_queue_bytes must be at least 1 MiB".into(),
            ));
        }
        Ok(())
    }
}
