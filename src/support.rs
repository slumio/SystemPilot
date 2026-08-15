//! Inspectable, local-only support bundles with mandatory secret redaction.
use crate::config_migration;
use crate::distributed::{redact_value, RedactionPolicy};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SUPPORT_BUNDLE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentState {
    Available,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRecord {
    pub name: String,
    pub state: ComponentState,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportBundleV1 {
    pub schema_version: u16,
    pub bundle_version: String,
    pub created_at_unix_nanos: u64,
    pub syspilot_version: String,
    pub build_commit: String,
    pub complete: bool,
    pub components: Vec<ComponentRecord>,
    pub data: Value,
    pub redaction: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CreateResult {
    pub path: PathBuf,
    pub bundle: SupportBundleV1,
}

fn now_ns() -> AppResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .map_err(|error| AppError::Validation(format!("system clock precedes Unix epoch: {error}")))
}

fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "authorization"
            | "cookie"
            | "set_cookie"
            | "password"
            | "passwd"
            | "secret"
            | "credential"
            | "credentials"
    ) || key.ends_with("_api_key")
        || key.ends_with("_token")
        || key.ends_with("_secret")
        || key.ends_with("_password")
        || key.ends_with("_credential")
}

fn redact_secrets(value: &mut Value, removed: &mut Vec<String>, path: &str) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let field = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if secret_key(key) {
                    *child = Value::String("[REDACTED]".into());
                    removed.push(field);
                } else {
                    redact_secrets(child, removed, &field);
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                redact_secrets(child, removed, &format!("{path}[{index}]"));
            }
        }
        _ => {}
    }
}

fn sanitize(mut value: Value, removed: &mut Vec<String>) -> Value {
    redact_secrets(&mut value, removed, "");
    redact_value(&mut value, &RedactionPolicy::default());
    value
}

fn read_json_component(
    name: &str,
    path: &Path,
    components: &mut Vec<ComponentRecord>,
    removed: &mut Vec<String>,
) -> Option<Value> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            components.push(ComponentRecord {
                name: name.into(),
                state: ComponentState::Unavailable,
                detail: "source file does not exist".into(),
            });
            return None;
        }
        Err(error) => {
            components.push(ComponentRecord {
                name: name.into(),
                state: ComponentState::Failed,
                detail: format!("could not read source file: {error}"),
            });
            return None;
        }
    };
    match serde_json::from_slice(bytes.as_slice()) {
        Ok(value) => {
            components.push(ComponentRecord {
                name: name.into(),
                state: ComponentState::Available,
                detail: "included after mandatory redaction".into(),
            });
            Some(sanitize(value, removed))
        }
        Err(error) => {
            components.push(ComponentRecord {
                name: name.into(),
                state: ComponentState::Failed,
                detail: format!("source file contains malformed JSON: {error}"),
            });
            None
        }
    }
}

pub fn create(
    syspilot_home: &Path,
    daemon_health_path: &Path,
    destination: Option<&Path>,
) -> AppResult<CreateResult> {
    let created_at = now_ns()?;
    let mut components = Vec::new();
    let mut removed = Vec::new();
    let configuration = read_json_component(
        "configuration",
        &syspilot_home.join("config.json"),
        &mut components,
        &mut removed,
    );
    let daemon_health = read_json_component(
        "daemon_health",
        daemon_health_path,
        &mut components,
        &mut removed,
    );
    components.push(ComponentRecord {
        name: "system".into(),
        state: ComponentState::Available,
        detail: "included without process arguments, environment, usernames, paths, or addresses"
            .into(),
    });
    removed.sort();
    removed.dedup();
    let complete = components
        .iter()
        .all(|component| component.state == ComponentState::Available);
    let bundle = SupportBundleV1 {
        schema_version: SUPPORT_BUNDLE_SCHEMA_VERSION,
        bundle_version: "SupportBundleV1".into(),
        created_at_unix_nanos: created_at,
        syspilot_version: env!("CARGO_PKG_VERSION").into(),
        build_commit: env!("SYSPILOT_BUILD_COMMIT").into(),
        complete,
        components,
        data: json!({
            "system": {
                "architecture": std::env::consts::ARCH,
                "operating_system": std::env::consts::OS,
            },
            "configuration": configuration,
            "daemon_health": daemon_health,
        }),
        redaction: removed,
    };
    let path = destination.map(Path::to_path_buf).unwrap_or_else(|| {
        syspilot_home
            .join("support")
            .join(format!("support-{created_at}.json"))
    });
    let bytes = serde_json::to_vec_pretty(&bundle)
        .map_err(|error| AppError::Protocol(format!("could not encode support bundle: {error}")))?;
    config_migration::atomic_write(&path, &bytes)?;
    Ok(CreateResult { path, bundle })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn written_bundle_removes_credentials_and_sensitive_context() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::write(
            home.join("config.json"),
            br#"{"gemini_api_key":"top-secret","fleet":{"bearer_token":"fleet-secret"},"nested":{"password":"pw","command":"/usr/bin/tool --token raw"}}"#,
        )
        .unwrap();
        let health = directory.path().join("health.json");
        fs::write(
            &health,
            br#"{"state":"ready","authorization":"Bearer hidden"}"#,
        )
        .unwrap();
        let destination = directory.path().join("bundle.json");

        let result = create(&home, &health, Some(&destination)).unwrap();
        assert!(result.bundle.complete);
        let written = fs::read_to_string(&destination).unwrap();
        for secret in [
            "top-secret",
            "fleet-secret",
            "Bearer hidden",
            "/usr/bin/tool",
            " raw",
        ] {
            assert!(!written.contains(secret), "bundle leaked {secret}");
        }
        assert!(written.contains("[REDACTED]"));
        assert_eq!(
            fs::metadata(destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn malformed_component_is_visible_and_bundle_is_partial() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::write(home.join("config.json"), b"{").unwrap();
        let destination = directory.path().join("bundle.json");

        let result = create(
            &home,
            &directory.path().join("missing-health.json"),
            Some(&destination),
        )
        .unwrap();
        assert!(!result.bundle.complete);
        assert!(result.bundle.components.iter().any(|component| {
            component.name == "configuration" && component.state == ComponentState::Failed
        }));
        assert!(result.bundle.components.iter().any(|component| {
            component.name == "daemon_health" && component.state == ComponentState::Unavailable
        }));
        assert!(destination.exists());
        assert!(!fs::read_to_string(destination)
            .unwrap()
            .contains(&directory.path().display().to_string()));
    }

    #[test]
    fn secret_key_variants_are_redacted_at_every_nesting_depth() {
        let sentinel = "never-emit-this-secret";
        for key in [
            "authorization",
            "PASSWORD",
            "service_api_key",
            "access_token",
            "client_secret",
            "db_password",
            "worker_credential",
        ] {
            let mut removed = Vec::new();
            let value = sanitize(
                serde_json::json!({"outer": [{key: sentinel}], "safe": 7}),
                &mut removed,
            );
            let encoded = serde_json::to_string(&value).unwrap();
            assert!(!encoded.contains(sentinel), "failed to redact key {key}");
            assert!(encoded.contains("[REDACTED]"));
            assert_eq!(value["safe"], 7);
        }
    }
}
