//! Presentation boundary for machine-readable CLI output.
//!
//! Domain modules return typed values. Encoding failures remain typed and
//! visible instead of being converted to empty strings or empty JSON values.
use crate::error::{AppError, AppResult};
use serde::Serialize;
use serde_json::Value;

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
}
