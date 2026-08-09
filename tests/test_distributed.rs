use std::collections::BTreeMap;
use syspilot::distributed::{
    DistributedTelemetryConfig, ExportPolicy, ProcessAlertEngine, ProcessAlertRule,
    ProcessNameMatch, TelemetryEnvelope, TelemetryKind, TELEMETRY_SCHEMA_VERSION,
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
