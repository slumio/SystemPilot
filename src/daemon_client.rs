//! Typed, fail-closed client for the local SysPilot daemon socket.

use crate::error::{AppError, AppResult};
use serde_json::Value;
use std::io::{Read, Write};
use std::time::Duration;

pub fn request(request: &str, timeout: Duration) -> AppResult<Value> {
    let mut stream = crate::config::connect_daemon()
        .map_err(|error| AppError::io("connect to daemon socket", error))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| AppError::io("set daemon read timeout", error))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| AppError::io("set daemon write timeout", error))?;
    let document = serde_json::json!({"request": request});
    stream
        .write_all(document.to_string().as_bytes())
        .map_err(|error| AppError::io("write daemon request", error))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| AppError::io("read daemon response", error))?;
    decode_response(&response)
}

pub fn events() -> AppResult<Value> {
    request("events", Duration::from_millis(750))
}
pub fn process_tree() -> AppResult<Value> {
    request("process_tree", Duration::from_millis(500))
}

fn decode_response(response: &str) -> AppResult<Value> {
    if response.trim().is_empty() {
        return Err(AppError::Protocol(
            "daemon returned an empty response".to_string(),
        ));
    }
    serde_json::from_str(response)
        .map_err(|error| AppError::Protocol(format!("malformed daemon response: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_empty_and_malformed_responses() {
        assert!(decode_response("").is_err());
        assert!(decode_response("not-json").is_err());
    }
    #[test]
    fn accepts_json_response() {
        let value = decode_response(r#"{"status":"ok","processes":[]}"#).unwrap();
        assert_eq!(value["status"], "ok");
    }
}
