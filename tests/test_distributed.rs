use std::collections::BTreeMap;
use syspilot::distributed::{
    preview_envelope, redact_value, DistributedTelemetryConfig, ExportPolicy, ProcessAlertEngine,
    ProcessAlertRule, ProcessNameMatch, RedactionPolicy, TelemetryEnvelope, TelemetryKind,
    TELEMETRY_SCHEMA_VERSION,
};

fn envelope() -> TelemetryEnvelope {
    TelemetryEnvelope {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        message_id: "node-a-1".into(),
        node_id: "node-a".into(),
        sequence: 42,
        observed_at_unix_nanos: 1_700_000_000_000_000_000,
        kind: TelemetryKind::SystemSnapshot,
        payload: serde_json::json!({"load_avg": "0.12 0.10 0.08"}),
        attributes: BTreeMap::new(),
    }
}

#[test]
fn valid_envelope_encodes_as_json() {
    let bytes = envelope()
        .encode_json()
        .expect("valid envelope must encode");
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["node_id"], "node-a");
    assert_eq!(value["sequence"], 42);
}

#[test]
fn envelope_rejects_unknown_schema() {
    let mut message = envelope();
    message.schema_version = 99;
    assert!(message.validate().is_err());
}

#[test]
fn export_policy_requires_queue_capacity_for_a_batch() {
    let policy = ExportPolicy {
        max_queue_messages: 10,
        batch_size: 11,
        ..Default::default()
    };
    assert!(policy.validate().is_err());
}

#[test]
fn enabled_export_requires_endpoint_and_node_identity() {
    let config = DistributedTelemetryConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn exact_and_prefix_process_rules_are_evaluated() {
    let engine = ProcessAlertEngine::new(vec![
        ProcessAlertRule {
            id: "postgres".into(),
            process_name: "postgres".into(),
            match_type: ProcessNameMatch::Exact,
            enabled: true,
            labels: BTreeMap::new(),
        },
        ProcessAlertRule {
            id: "api".into(),
            process_name: "api-".into(),
            match_type: ProcessNameMatch::Prefix,
            enabled: true,
            labels: BTreeMap::new(),
        },
    ])
    .unwrap();
    assert_eq!(engine.evaluate("postgres", 10, 1, "EXEC", 1).len(), 1);
    assert_eq!(engine.evaluate("api-worker", 11, 1, "EXEC", 1).len(), 1);
    assert!(engine.evaluate("redis", 12, 1, "EXEC", 1).is_empty());
}

#[test]
fn redaction_removes_nested_sensitive_fields_but_preserves_shape() {
    let mut payload = serde_json::json!({
        "process": { "name": "api", "args": ["--token", "secret"], "env": ["PASSWORD=secret"], "executable_path": "/home/alice/api" },
        "connection": { "remote_ip": "10.0.0.7" },
        "source": "password = secret",
        "safe": "retained",
        "summary": "mounted /dev/sda1 at /srv/data",
        "peer": "connected to 10.0.0.8"
    });
    redact_value(&mut payload, &RedactionPolicy::default());
    assert_eq!(payload["process"]["args"], "[REDACTED]");
    assert_eq!(payload["process"]["env"], "[REDACTED]");
    assert_eq!(payload["process"]["executable_path"], "[REDACTED]");
    assert_eq!(payload["connection"]["remote_ip"], "[REDACTED]");
    assert_eq!(payload["source"], "[REDACTED]");
    assert_eq!(payload["safe"], "retained");
    assert_eq!(payload["summary"], "[REDACTED]");
    assert_eq!(payload["peer"], "[REDACTED]");
}

#[test]
fn individual_redaction_classes_can_be_disabled() {
    let policy = RedactionPolicy {
        paths: false,
        ..Default::default()
    };
    let mut payload = serde_json::json!({"path":"/srv/api", "username":"alice"});
    redact_value(&mut payload, &policy);
    assert_eq!(payload["path"], "/srv/api");
    assert_eq!(payload["username"], "[REDACTED]");
}

#[test]
fn preview_is_redacted_even_when_export_is_disabled() {
    let config = DistributedTelemetryConfig {
        node_id: "node-a".into(),
        attributes: BTreeMap::from([
            ("username".into(), "alice".into()),
            ("environment".into(), "prod".into()),
        ]),
        ..Default::default()
    };
    let envelope = preview_envelope(
        &config,
        TelemetryKind::ProcessSnapshot,
        &serde_json::json!({"pid":42,"env":["TOKEN=secret"],"name":"api"}),
    )
    .unwrap();
    assert_eq!(envelope.payload["env"], "[REDACTED]");
    assert_eq!(envelope.payload["name"], "api");
    assert_eq!(envelope.attributes["username"], "[REDACTED]");
    assert_eq!(envelope.attributes["environment"], "prod");
    assert_eq!(envelope.sequence, 0);
}
