# SysPilot Architecture

This document describes the current Rust implementation. For setup, operation, distributed telemetry, alert rules, and troubleshooting, use the [project README](README.md). The [documentation index](docs/README.md) identifies historical material that does not describe the active system.

## Runtime flow

```text
Linux procfs ───────────────────────────┐
Linux Netlink Process Connector ────────┼──> daemon.rs ──> local UNIX socket
                                        │        │
CLI / TUI ──> main.rs ──> telemetry.rs ─┼──> causal_engine.rs ──> AI provider
                                        │        │
                                        │        └──> distributed HTTP exporter
                                        │                 └──> process_alert envelopes
```

## Components and responsibilities

| Component | Responsibility |
|---|---|
| `src/main.rs` | CLI command handling, configuration actions, RCA-context construction, and user-visible errors. |
| `src/config.rs` | Typed configuration, validation, persistence, environment key overrides, AI transport settings, telemetry settings, and alert rules. |
| `src/daemon.rs` | Initial procfs scan, Netlink lifecycle events, local UNIX socket service, and configured alert evaluation. |
| `src/distributed.rs` | Versioned envelopes, bounded queue, batching, retry policy, HTTP delivery, and exact/prefix process-name matching. |
| `src/telemetry.rs` | Linux procfs process and system data. |
| `src/causal_engine.rs` | Graph construction, reverse causal traversal, and graph exports. |
| `src/ai.rs` | Gemini, Ollama, and SysPilot API streaming requests with provider failure handling. |
| `src/codebase.rs` | Local source chunking and codebase index/query support. |
| `src/profiler.rs` | Optional process profiling data. |
| `src/ui/` | Terminal monitor and Markdown stream renderer. |
| `crates/syspilot-abi` | Shared telemetry ABI types. |
| `crates/syspilot-collector` | Bounded collector primitives and fleet database schema invariants. |
| `deploy/fleet/` | PostgreSQL fleet-control migration and fail-closed setup tooling. |

## Distributed telemetry boundary

The daemon persists redacted `process_lifecycle` and configured `process_alert` envelopes before asynchronous HTTP delivery, so collector latency never blocks the Netlink loop. Acknowledged records leave the spool; failures remain durable for replay. Collectors use `node_id`, `message_id`, and monotonic `sequence` to deduplicate deliveries and report gaps.

The optional fleet database exists only behind the collector/control-plane API. Agents never receive database credentials or issue SQL. PostgreSQL row-level security provides defense in depth, but request authorization must still derive and set the verified tenant inside every transaction. See [Fleet control-plane database](docs/FLEET_CONTROL_PLANE.md).

The collector endpoint, authentication token, node ID, attributes, batch size, queue capacity, retry policy, and alert rules are configuration, not source-code deployment values. See [Distributed telemetry and alerts](README.md#distributed-telemetry-and-alerts).

## RCA data boundary

RCA starts with data collected by the selected command path: procfs telemetry, optional graph evidence, optional profiler or eBPF data, codebase context, and configured alert/event context when available. AI output is requested to distinguish evidence from inference and report confidence. It is not a substitute for validating changes in the target environment.

## Build and verification

```bash
cargo build --release
cargo fmt --all -- --check
cargo test --workspace
```

Use `cargo build --release` to create the release binary at `target/release/syspilot`.

## Security and permissions

SysPilot uses normal Linux process permissions. Data for protected processes can be absent. Optional tracing and profiling require their own tools and permissions. Treat external AI prompts and telemetry collector payloads as data leaving the host; configure them according to your security policy.
