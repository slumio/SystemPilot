# SysPilot telemetry protocol v1

This document is the public collector contract for `TelemetryEnvelope` schema version 1. The contract is transport-neutral JSON; the bundled agent sends a JSON array of envelopes in an HTTP `POST` request. Unknown object fields must be ignored so compatible fields can be added within schema v1.

## Envelope

Every record contains:

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | unsigned integer | Envelope schema. This document defines value `1`. |
| `message_id` | string | Agent-generated record identity. Treat as opaque. |
| `node_id` | string | Configured host identity. |
| `sequence` | unsigned integer | Per-process monotonic publishing sequence. A restart can reset it in the current agent. |
| `observed_at_unix_nanos` | unsigned integer | Observation time since the Unix epoch. |
| `kind` | string | Payload discriminator. |
| `payload` | JSON value | Kind-specific data. It is never `null`. |
| `attributes` | string map | Operator-configured labels after redaction. |

A `process_alert` payload includes `instance_id`, `state`, and optional `previous_state`; additions are backward-compatible within envelope schema v1. Defined kinds are `process_lifecycle`, `process_alert`, `process_snapshot`, `system_snapshot`, `causal_graph`, and `health`. Collectors must retain or safely reject unknown kinds; they must not decode an unknown kind as a known payload.

## HTTP ingestion

The agent sends `Content-Type: application/json` and optionally `Authorization: Bearer <token>`. A 2xx response with an empty body acknowledges the complete submitted batch for compatibility with schema-v1 collectors. A non-empty 2xx response must contain `accepted_message_ids`, optional `highest_accepted_sequence`, `rejected_records` with reasons, and optional `retry_after_ms`. Unknown IDs, duplicate decisions, inconsistent high-water sequences, and malformed responses do not acknowledge records. Non-2xx responses and transport failures are retried with exponential backoff and jitter. Server-directed retry timing is honored. Records are redacted before being atomically persisted and remain in the owner-only disk spool until explicitly acknowledged.

Collectors should deduplicate by `(node_id, message_id)`. Sequence gaps are evidence of loss or quarantine and must be reported; sequences do not reset during normal agent restarts. Collector clocks must not replace `observed_at_unix_nanos`; ingestion time should be stored separately.

## Privacy boundary

Redaction is applied before an envelope enters the export queue. The default policy removes command arguments, environment variables, filesystem paths, usernames, IP addresses, and source snippets. Operators can inspect an envelope without enabling export:

```bash
syspilot config telemetry preview
syspilot config telemetry preview <pid-or-name>
```

Bearer tokens are transport credentials and never appear in envelopes or previews.

## Compatibility

Collectors supporting schema v1 must:

1. validate required fields and reject malformed records without crashing;
2. ignore unknown object fields;
3. report sequence gaps and deduplicate replayed records without assuming exactly-once delivery;
4. keep the original envelope when a payload kind is unknown;
5. impose explicit request and storage bounds.

An incompatible change to required field meaning, type, or kind interpretation requires a new envelope schema version. Payload additions within a known kind must be optional for v1 readers.
