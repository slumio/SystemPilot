> **Historical documentation notice**
>
> This document is retained as historical design reference. It is not valid for the current Rust application. Use the current [documentation index](../README.md), [project README](../../README.md), and [architecture guide](../../ARCHITECTURE.md) for build, configuration, deployment, and behavior.

# Distributed Telemetry Layout

This is the staged design for distributed telemetry. The current repository implements the local daemon and the shared message contract in `src/distributed.rs`; collectors, durable queues, and network exporters are planned work and are not enabled by default.

## Topology

```text
Linux host                         Control plane
──────────                         ─────────────
syspilotd                          Ingest gateway
  │ Netlink + /proc                     │ authenticate, rate-limit
  ▼                                    ▼
local process/event store  ──TLS──► durable ingest queue
  │                                    │
  ├─ local UNIX socket                 ├─ stream processor / enrichers
  └─ future exporter                   ├─ time-series store
       │ batches                       └─ causal graph store + query API
       └─ disk-backed retry queue
```

## Boundary contracts

Every exported record is a `TelemetryEnvelope` with a fixed schema version, node ID, monotonic per-node sequence, observation timestamp, kind, payload, and attributes. The collector deduplicates `(node_id, sequence)` and acknowledges only after durable enqueue. This provides at-least-once delivery without double-counting.

| Layer | Responsibility | Failure behavior |
|---|---|---|
| Agent | Capture, redact, batch, persist retry queue | Drop oldest low-priority data only after queue limit; emit a health event |
| Gateway | mTLS auth, tenant routing, limits | Reject invalid schema/identity with a typed response; no silent discard |
| Queue | Durable hand-off | Backpressure gateway before storage exhaustion |
| Processor | Normalize, enrich, deduplicate | Idempotent writes keyed by node and sequence |
| Query | TUI/API reads | Surface staleness and missing sequence ranges |

## Rollout phases

1. **Local-only (current):** daemon and monitor communicate through the UNIX socket.
2. **Contract and health:** emit the versioned envelope to a local append-only spool; add exporter health to `status`.
3. **Single collector:** HTTPS/mTLS batch ingest with acknowledgements, bounded retries, and per-node rate limits.
4. **Multi-region:** regional gateways, durable queue replication, and a query layer that exposes freshness and partial results.

## Security and operational rules

- mTLS node identity and short-lived certificates; never send API keys in telemetry.
- Redact environment values, command arguments, and file paths according to an explicit policy before enqueue.
- Bound memory, batch size, queue size, retry backoff, and request deadlines. `ExportPolicy` validates these limits before an exporter is started.
- Keep capture independent of export: a collector outage must not block the local daemon or monitor.
- Include collector health, spool usage, last acknowledged sequence, and dropped-record count in every node's local status.
