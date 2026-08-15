//! Presentation boundary for machine-readable CLI output.
//!
//! Domain modules return typed values. Encoding failures remain typed and
//! visible instead of being converted to empty strings or empty JSON values.
use crate::error::{AppError, AppResult};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub const OUTPUT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Degraded,
    Error,
}

impl Outcome {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Error => 1,
            Self::Degraded => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    Degraded,
    Unavailable,
    Misconfigured,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticV1 {
    pub component: String,
    pub state: CapabilityState,
    pub detail: String,
    pub impact: String,
    pub recovery_command: Option<String>,
    pub fallback_used: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputEnvelopeV1<T> {
    pub schema_version: u16,
    pub command: String,
    pub generated_at_unix_nanos: u64,
    pub outcome: Outcome,
    pub data: T,
    pub diagnostics: Vec<DiagnosticV1>,
}

impl<T> OutputEnvelopeV1<T> {
    pub fn new(
        command: impl Into<String>,
        outcome: Outcome,
        data: T,
        diagnostics: Vec<DiagnosticV1>,
    ) -> AppResult<Self> {
        let generated_at_unix_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                AppError::Validation(format!("system clock precedes Unix epoch: {error}"))
            })?
            .as_nanos() as u64;
        Ok(Self {
            schema_version: OUTPUT_SCHEMA_VERSION,
            command: command.into(),
            generated_at_unix_nanos,
            outcome,
            data,
            diagnostics,
        })
    }
}

pub fn pretty<T: Serialize + ?Sized>(value: &T) -> AppResult<String> {
    serde_json::to_string_pretty(value)
        .map_err(|error| AppError::Protocol(format!("could not encode JSON output: {error}")))
}

pub fn parse_value(document: &str, context: &'static str) -> AppResult<Value> {
    serde_json::from_str(document)
        .map_err(|error| AppError::Protocol(format!("could not decode {context}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::{Error, Serializer};

    struct Broken;
    impl Serialize for Broken {
        fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(S::Error::custom("intentional failure"))
        }
    }

    #[test]
    fn encoding_failure_is_visible() {
        assert!(pretty(&Broken)
            .unwrap_err()
            .to_string()
            .contains("intentional failure"));
    }

    #[test]
    fn malformed_json_is_visible() {
        assert!(parse_value("{", "causal chain")
            .unwrap_err()
            .to_string()
            .contains("causal chain"));
    }

    #[test]
    fn envelope_v1_has_stable_outcome_and_exit_contract() {
        let envelope = OutputEnvelopeV1::new(
            "doctor",
            Outcome::Degraded,
            serde_json::json!({"ready": true}),
            vec![],
        )
        .unwrap();
        let value: Value = serde_json::from_str(&pretty(&envelope).unwrap()).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["command"], "doctor");
        assert_eq!(value["outcome"], "degraded");
        assert_eq!(envelope.outcome.exit_code(), 2);
    }
}
