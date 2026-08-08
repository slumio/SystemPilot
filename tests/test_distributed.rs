use std::collections::BTreeMap;
use syspilot::distributed::{
    ExportPolicy, TelemetryEnvelope, TelemetryKind, TELEMETRY_SCHEMA_VERSION,
};

fn envelope() -> TelemetryEnvelope {
    TelemetryEnvelope {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        message_id: "a3a0cf2d-9197-47ef-a4b3-4b49ea525ca1".into(),
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
fn export_policy_rejects_unboundedly_small_queue() {
    let policy = ExportPolicy {
        max_queue_bytes: 10,
        ..Default::default()
    };
    assert!(policy.validate().is_err());
}
