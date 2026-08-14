# SysPilot

**Evidence-first Linux diagnostics with local-first code context and provider-choice AI reasoning.**

SysPilot is a Rust command-line application for investigating Linux process activity and failed commands. It combines local procfs data, an optional Netlink daemon, a causal graph, optional profiling/tracing, codebase search, streaming AI analysis, and optional HTTP telemetry export.

### Why SysPilot

- **Evidence before inference:** it gathers process, scheduler, stack, and dependency evidence before asking an AI model for a causal hypothesis.
- **Local-first context:** source chunking and vector retrieval can run locally with Ollama/Qwen, so codebase embeddings do not require a cloud provider.
- **No model lock-in:** use a local Ollama model, Gemini, or a compatible SysPilot API for the final explanation.
- **Built for systems contributors:** the core is Rust, Linux-native, and intentionally split into small modules for procfs, Netlink, profiling, causal analysis, and transport.

If you want to help build a transparent alternative to opaque "AI ops" tools, start with the [roadmap](docs/ROADMAP.md), read [CONTRIBUTING.md](CONTRIBUTING.md), and open a focused issue or pull request.

This document describes the behavior implemented in this repository. It does not promise a specific diagnostic result, latency, CPU limit, or root cause. AI output is an evidence-based hypothesis and must be reviewed before operational changes are made.

## Contents

- [Why SysPilot](#why-syspilot)
- [What is available](#what-is-available)
- [Requirements](#requirements)
- [Quick start](#quick-start)
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

## Quick start

### Recommended: install with Cargo

```bash
git clone https://github.com/slumio/SystemPilot.git syspilot
cd syspilot
cargo install --path .
syspilot setup
syspilot status
syspilot ask "Why is Linux load average high?"
```

`setup` creates the local configuration and shell hook, offers Gemini or Ollama setup, and installs a copy of the running binary in `~/.local/bin` when needed. It does not change your shell profile. If `~/.local/bin` is not already on your `PATH`, it prints the exact export line to add.

### Build from source instead

```bash
cargo build --release
./target/release/syspilot setup
```

Use `cargo build --release` for a portable release binary.

### Individual installation actions

```bash
# Create configuration, logs, and the Bash hook only
syspilot install

# Copy the running binary to ~/.local/bin without overwriting an existing copy
syspilot install --binary

# Replace an existing user-local copy
syspilot install --binary --force

# Remove only the user-local copy
syspilot uninstall --binary
```

To enable shell command capture, add this line to your Bash profile, then open a new terminal:

```bash
source ~/.syspilot/syspilot.sh
```

Check the installation and next actions:

```bash
syspilot status
syspilot --version
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

### Local Qwen embeddings with a remote explanation provider

Chunking is local. To keep vector indexing and retrieval local while continuing
to use Gemini for the final explanation, install a local embedding model and
configure it independently:

```bash
ollama pull qwen3-embedding:0.6b
./target/release/syspilot config set embedding_provider ollama
./target/release/syspilot config set embedding_model qwen3-embedding:0.6b
./target/release/syspilot index --force
```

Keep `active_provider` set to `gemini`. Changing an embedding provider or model
automatically rebuilds the local vector index on its next use.

## Daemon and monitor

Start the daemon in a dedicated terminal or under a service manager:

```bash
./target/release/syspilot daemon
```

The daemon performs an initial procfs scan, attempts to subscribe to the Linux Netlink Process Connector, and serves a local UNIX socket. User installs use `$XDG_RUNTIME_DIR/syspilot/syspilot.sock` (with a UID-scoped `/tmp` fallback); the packaged system service uses `/run/syspilot/syspilot.sock`. If Netlink subscription fails, it logs the problem and continues with the initial snapshot; it will not receive live lifecycle events.

In another terminal:

```bash
./target/release/syspilot events
./target/release/syspilot monitor
```

`events` reads the currently queued lifecycle events from the daemon. The queue is consumed by that request, so a later `events` request only shows events received afterward.

The terminal monitor supports `Tab`, arrow keys or `j`/`k`, `e` or Enter, `s`, `r`, `x`, and `q`. Signal actions depend on normal Linux process permissions.

For packaged systemd deployment, install [the service unit](deploy/syspilot.service), then run:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now syspilot
systemctl status syspilot
```

The system service runs as the dedicated `syspilot` user. Add interactive users to the `syspilot` group if they need to query its socket. For package layout and release requirements, see [Packaging](docs/PACKAGING.md).

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
cargo clippy --workspace --all-targets -- -D warnings
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
