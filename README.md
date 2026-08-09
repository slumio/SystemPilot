# SysPilot

SysPilot is a Rust command-line application for investigating Linux process activity and failed commands. It combines local procfs data, an optional Netlink daemon, a causal graph, optional profiling/tracing, codebase search, streaming AI analysis, and optional HTTP telemetry export.

This document describes the behavior implemented in this repository. It does not promise a specific diagnostic result, latency, CPU limit, or root cause. AI output is an evidence-based hypothesis and must be reviewed before operational changes are made.

## Contents

- [What is available](#what-is-available)
- [Requirements](#requirements)
- [Build and first run](#build-and-first-run)
- [AI setup](#ai-setup)
- [Diagnostics and RCA](#diagnostics-and-rca)
- [Daemon and monitor](#daemon-and-monitor)
- [Distributed telemetry and alerts](#distributed-telemetry-and-alerts)
- [Configuration reference](#configuration-reference)
- [Limits and troubleshooting](#limits-and-troubleshooting)
- [Development](#development)

## What is available

| Area | What SysPilot does | What you provide |
|---|---|---|
| Process diagnostics | Reads Linux procfs data for a named process or PID. | A visible process and Linux procfs access. |
| Causal analysis | Builds a process/resource graph and traces a reverse path from the selected PID. | `--pid` and `--causal`. |
| Profiling | Includes process stack/profile data when requested. | `--deep`; system tooling may affect the result. |
| eBPF tracing | Runs optional syscall tracing when requested. | `--ebpf`, `bpftrace`, and the required privileges. |
| AI RCA | Streams analysis from Gemini, Ollama, or the configured SysPilot API. | A selected provider plus its valid credentials or local service. |
| Daemon | Subscribes to Netlink process lifecycle events and serves local process/event data. | Linux kernel support for the Process Connector. |
| Distributed telemetry | POSTs batches of lifecycle and matching alert envelopes to your HTTP collector. | An HTTPS/HTTP endpoint, node ID, and optional bearer token. |
| Process alerts | Emits a `process_alert` telemetry envelope for matching process names. | Alert rules. This does not send email, Slack, PagerDuty, or kill processes. |

## Requirements

### Required for build

- A stable Rust toolchain that supports Rust edition 2021.
- Cargo.

### Required for Linux diagnostics

- Linux with `/proc` mounted.
- Permission to inspect the target process. Data for processes owned by another user can be unavailable or incomplete.

### Optional tools

| Tool | Used by | If absent |
|---|---|---|
| `curl` | Gemini, Ollama, and SysPilot streaming AI requests | AI requests fail with a visible command error. |
| `bpftrace` | `explain --ebpf` | eBPF collection is unavailable. |
| `perf` | Deep profiling where supported | Profile data can be limited or unavailable. |
| systemd | Service management | Use the provided unit only on systemd hosts. |

## Build and first run

```bash
git clone https://github.com/your-org/syspilot.git
cd syspilot
cargo build --release
./target/release/syspilot --help
```

`./build_rust.sh` is an alternative local release build helper. It enables `target-cpu=native`; use `cargo build --release` for a binary intended for a different CPU.

### Install shell integration

```bash
./target/release/syspilot install
source ~/.syspilot/syspilot.sh
```

The install command creates `~/.syspilot/config.json` and `~/.syspilot/syspilot.sh`. Source the shell script from `~/.bashrc` or `~/.zshrc` to capture command history for `syspilot explain` without a PID.

Check the installation:

```bash
./target/release/syspilot status
```

## AI setup

SysPilot does not send data to an AI provider until you run `ask` or `explain`. The selected provider receives the prompt context created by that command. Review that context before using an external provider in an environment with sensitive data.

### Gemini

```bash
./target/release/syspilot config set-key gemini YOUR_GEMINI_API_KEY
./target/release/syspilot provider gemini
./target/release/syspilot ask "Explain Linux load average"
```

`GEMINI_API_KEY` overrides the configured Gemini key for the current process environment.

### Ollama

Start Ollama separately, then configure SysPilot:

```bash
./target/release/syspilot provider ollama
./target/release/syspilot config set-url ollama http://localhost:11434
./target/release/syspilot pull llama3 --set-active
./target/release/syspilot ask "Explain Linux load average"
```

The configured Ollama URL must be a valid URL. SysPilot reports HTTP failures, connection failures, and empty streamed responses as errors.

### SysPilot API

```bash
./target/release/syspilot config set-key syspilot YOUR_API_KEY
./target/release/syspilot provider syspilot
```

The endpoint and AI request timeouts are stored in `~/.syspilot/config.json`. The shipped values are defaults, not fixed deployment requirements.

## Diagnostics and RCA

### Explain a failed shell command

```bash
./target/release/syspilot explain
```

This reads the most recent captured command from `~/.syspilot/context.log`. If shell integration has not recorded a command, SysPilot reports that no recent command was found.

### Inspect one process

```bash
./target/release/syspilot explain --pid 1234
./target/release/syspilot explain --pid nginx --deep
```

`--pid` accepts a numeric PID or a process name. A name is resolved from procfs; if there is no match, the command stops with an error.

### Causal RCA

```bash
./target/release/syspilot explain --pid 1234 --causal
./target/release/syspilot explain --pid 1234 --causal --ebpf
```

`--causal` requires `--pid`. SysPilot includes the causal graph and codebase context when indexing is enabled. It writes DOT and HTML graph reports below `~/syspilot_reports` when graph export succeeds.

The RCA prompt requires the provider to separate observed evidence, causal hypotheses, competing explanations, confidence, and remediation. It cannot prove causality from incomplete telemetry.

Useful options:

| Option | Effect |
|---|---|
| `--deep` | Includes deeper process profiling in a non-causal process analysis. |
| `--ebpf` | Requests optional eBPF tracing; privileges and `bpftrace` are required. |
| `--no-index` | Does not query the local codebase index. |
| `--number N` | Uses the Nth most recent captured failed command. |

### Codebase index

```bash
./target/release/syspilot index
./target/release/syspilot ask "Where is process telemetry collected?"
```

Run `index --force` after source changes when you want a fresh local index.

## Daemon and monitor

Start the daemon in a dedicated terminal or under a service manager:

```bash
./target/release/syspilot daemon
```

The daemon performs an initial procfs scan, attempts to subscribe to the Linux Netlink Process Connector, and serves a local UNIX socket at `/tmp/syspilot.sock`. If Netlink subscription fails, it logs the problem and continues with the initial snapshot; it will not receive live lifecycle events.

In another terminal:

```bash
./target/release/syspilot events
./target/release/syspilot monitor
```

`events` reads the currently queued lifecycle events from the daemon. The queue is consumed by that request, so a later `events` request only shows events received afterward.

The terminal monitor supports `Tab`, arrow keys or `j`/`k`, `e` or Enter, `s`, `r`, `x`, and `q`. Signal actions depend on normal Linux process permissions.

For systemd deployment and health checks, see [Daemon Reliability](docs/RELIABILITY.md).

## Distributed telemetry and alerts

Distributed telemetry is disabled by default. When enabled, the daemon sends JSON arrays of `TelemetryEnvelope` objects to the configured endpoint using HTTP POST with `Content-Type: application/json`. When a bearer token is configured, it sends `Authorization: Bearer TOKEN`.

### Configure export

```bash
./target/release/syspilot config telemetry enable https://collector.example/v1/telemetry node-a YOUR_TOKEN
./target/release/syspilot config telemetry show
```

Restart the daemon after changing telemetry settings:

```bash
./target/release/syspilot daemon
```

Disable export:

```bash
./target/release/syspilot config telemetry disable
```

The exporter uses a bounded in-memory queue. It batches messages, retries failed batches according to `export_policy`, and logs an error when a batch is dropped after its retry budget. It does not persist unsent telemetry to disk and therefore does not guarantee delivery during process termination, queue overflow, or prolonged collector failure.

### Add process-name alerts

```bash
./target/release/syspilot config alert add postgres-exit exact postgres
./target/release/syspilot config alert add api-worker prefix api-
./target/release/syspilot config alert list
./target/release/syspilot config alert remove postgres-exit
```

- `exact` matches the complete process name reported by the kernel/procfs.
- `prefix` matches process names that begin with the configured text.
- A matching lifecycle event produces a `process_alert` envelope in addition to the `process_lifecycle` envelope.
- Rules are evaluated for lifecycle events. They do not poll for process health and do not perform any local notification or remediation.

### Envelope contract

Each envelope contains `schema_version`, `message_id`, `node_id`, monotonically increasing `sequence`, `observed_at_unix_nanos`, `kind`, `payload`, and optional `attributes`.

The collector must accept a JSON array of these envelopes and return an HTTP success status. SysPilot currently emits `process_lifecycle` and `process_alert` from the daemon. The schema also reserves other kinds for future use; they are not emitted by the current daemon.

### Full telemetry configuration

The CLI handles enabling export and basic rules. Edit `~/.syspilot/config.json` to set deployment attributes, labels, and delivery policy:

```json
{
  "distributed_telemetry": {
    "enabled": true,
    "endpoint": "https://collector.example/v1/telemetry",
    "node_id": "node-a",
    "bearer_token": "replace-with-a-secret",
    "attributes": { "environment": "production", "region": "in-1" },
    "export_policy": {
      "batch_size": 256,
      "flush_interval_ms": 1000,
      "max_queue_messages": 4096,
      "retry_backoff_ms": 1000,
      "max_retries": 3,
      "request_timeout_ms": 10000
    },
    "process_alert_rules": [
      {
        "id": "postgres-exit",
        "process_name": "postgres",
        "match_type": "exact",
        "enabled": true,
        "labels": { "service": "database", "severity": "high" }
      }
    ]
  }
}
```

SysPilot validates the endpoint, node ID, rule IDs, and policy before starting the daemon or saving configuration. A duplicate alert-rule ID, invalid URL, zero timeout, or queue smaller than one batch prevents configuration from being accepted.

## Configuration reference

Configuration is stored at `~/.syspilot/config.json`. Do not commit this file when it contains credentials.

| Field | Meaning |
|---|---|
| `active_provider` | `gemini`, `ollama`, or `syspilot`. |
| `gemini_api_key`, `syspilot_api_key` | Provider credentials. Environment variables can override these keys. |
| `ollama_url`, `ollama_model` | Local Ollama connection and model. |
| `gemini_model`, `syspilot_model` | Selected remote model. |
| `ai_request_timeout_seconds`, `ai_connect_timeout_seconds` | AI HTTP limits. Values must be greater than zero. |
| `syspilot_url` | SysPilot API endpoint. |
| `distributed_telemetry` | Endpoint, identity, delivery policy, attributes, and alert rules. |

## Limits and troubleshooting

| Situation | What to check |
|---|---|
| `Could not find process PID` | Verify the process exists, spelling is correct, and your user can inspect it. |
| No live daemon events | Check daemon logs and kernel support/permission for `cn_proc`. The daemon can still serve its startup snapshot. |
| AI request fails | Confirm `curl` is installed, the provider is selected, credentials are valid, and the endpoint is reachable. |
| AI returns no usable content | The provider response did not contain a supported streamed payload. Check the endpoint/model and provider logs. |
| No distributed export | Run `config telemetry show`, verify the collector URL and token, then restart the daemon. |
| No process alert | Verify the kernel-reported process name with `cat /proc/PID/comm`, then choose `exact` or `prefix` accordingly. |
| Collector receives duplicates or gaps | Use `node_id` and `sequence` for deduplication/gap detection. Delivery is at-least-attempted, not exactly-once. |

For the complete document map and the status of historical material, see [Documentation](docs/README.md).

## Development

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets
```

Key modules:

- `src/main.rs`: CLI behavior and RCA context construction.
- `src/daemon.rs`: Netlink listener, local socket, lifecycle events, alert evaluation.
- `src/distributed.rs`: envelope contract, HTTP exporter, queue/retry policy, alert rules.
- `src/telemetry.rs`: procfs readers.
- `src/causal_engine.rs`: graph construction and causal traversal.
- `src/ai.rs`: provider requests and streamed output handling.
- `tests/`: integration tests.

## License

MIT. See [LICENSE](LICENSE).
