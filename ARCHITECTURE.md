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
| `crates/syspilot-collector` | Bounded collector primitives. |

## Distributed telemetry boundary

The daemon publishes `process_lifecycle` and configured `process_alert` envelopes. The exporter is asynchronous and bounded so collector latency does not block the Netlink loop. Delivery is HTTP POST to the configured endpoint; unsent messages are not persisted to disk. Collector outages, process termination, and queue pressure can therefore cause loss. Collectors should use `node_id` and monotonic `sequence` to deduplicate deliveries and detect gaps.

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
