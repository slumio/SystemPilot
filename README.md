# SysPilot

**Evidence-first Linux diagnostics with local-first code context and provider-choice AI reasoning.**

SysPilot is a Rust command-line application for investigating Linux process activity and failed commands. It combines local procfs data, an optional Netlink daemon, a causal graph, optional profiling/tracing, codebase search, streaming AI analysis, and optional HTTP telemetry export.

For a visual explanation of deployment choices, local data flow, security boundaries, and distributed telemetry, see [Architecture](ARCHITECTURE.md).

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
| `curl` | Ollama model downloads through `syspilot pull` | Model downloads fail with a visible command error. AI questions use the built-in Rust HTTP client. |
| `bpftrace` | `explain --ebpf` | eBPF collection is unavailable. |
| `perf` | Deep profiling where supported | Profile data can be limited or unavailable. |
| systemd | Service management | Use the provided unit only on systemd hosts. |

## Quick start

### One-command install

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/slumio/SystemPilot/dev/install.sh | sh
syspilot setup
syspilot status
syspilot ask "Why is Linux load average high?"
```

The installer requires Linux plus `curl`, `tar`, and SHA-256 tooling. It downloads the selected release binary, verifies its published checksum, and atomically updates `~/.local/bin/syspilot`. Missing or invalid release assets are hard failures; source installation is a separate, explicit workflow. Review [install.sh](install.sh) before piping it to a shell.

On x86_64 and ARM64, the installer first looks for an architecture-specific release archive and its SHA-256 file. It installs that binary only after verification. If a matching release asset or checksum is unavailable, installation fails without changing the existing binary. Set `SYSPILOT_VERSION=vX.Y.Z` to select a release instead of `latest`.

Generate native shell completions with:

```bash
syspilot completions bash
syspilot completions zsh
syspilot completions fish
```

`setup` starts a terminal configuration wizard for local-only, self-hosted collector, or hosted-fleet operation, followed by optional AI configuration and a masked confirmation screen. It validates the complete draft before writing, does not change your shell profile, and never echoes fleet credentials. Use `syspilot setup --line` over restricted terminals or `syspilot setup --check` to inspect terminal capability without changing configuration. `syspilot setup --tui` fails explicitly when a full-screen terminal is unavailable instead of silently falling back.

### Build from source instead

```bash
git clone https://github.com/slumio/SystemPilot.git syspilot
cd syspilot
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

The daemon performs an initial procfs scan, attempts to subscribe to the Linux Netlink Process Connector, and serves a local UNIX socket. User installs use the UID-scoped Linux abstract socket `@syspilot-UID`, which leaves no stale socket file. The packaged system service uses `/run/syspilot/syspilot.sock` for group-based access control. If Netlink subscription fails, it logs the problem and continues with the initial snapshot; it will not receive live lifecycle events.

Modern kernels require `CAP_NET_ADMIN` for Process Connector events. For a user-local binary, grant only that capability and restart the daemon:

```bash
sudo setcap cap_net_admin=ep "$(command -v syspilot)"
syspilot daemon
```

If the binary is replaced during an upgrade, apply `setcap` again. Packaged deployments should use the system service instead.

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

The exporter redacts each envelope before atomically appending it to an owner-only disk spool. The default bound is 512 MiB or seven days. Bounded in-memory notifications only wake the delivery worker; notification saturation cannot lose persisted records. Failed batches remain durable for replay across reconnects and daemon restarts, with exponential backoff, jitter, and collector-directed retry timing. Spool pressure, quarantine, persistence failures, retries, and collector rejections are visible in daemon health and fleet status.

### Add process-name alerts

```bash
./target/release/syspilot config alert add postgres-exit exact postgres
./target/release/syspilot config alert add api-worker prefix api-
./target/release/syspilot config alert list
./target/release/syspilot config alert remove postgres-exit
```

- `exact` matches the complete process name reported by the kernel/procfs.
- `prefix` matches process names that begin with the configured text.
- FORK or EXEC opens a persistent `firing` alert; EXIT resolves an existing alert. Repeated events in the same state are deduplicated.
- Alert state is atomically stored in `~/.syspilot/alerts-v1.json` with owner-only permissions and recovered after daemon restart.
- Use `syspilot alerts list`, `acknowledge`, `resolve`, and `suppress` to manage lifecycle state. Acknowledged alerts stay acknowledged until exit or explicit change; suppressed alerts require explicit release.
- Rules are deterministic and independent of AI. They do not execute remediation commands.

### Fleet control-plane database

Fleet inventory, ingestion deduplication, sequence gaps, shared cases, alert state, RBAC, retention, deletion requests, and immutable audit events use the optional PostgreSQL control-plane schema. Agents never connect to the database and local operation remains independent. See [Fleet control-plane database](docs/FLEET_CONTROL_PLANE.md) for fail-closed setup and mandatory tenant transaction rules.

### Local collection with AWS reasoning

Process collection, redaction, and the durable retry spool always run on the
Linux host. When an operator enables export after reviewing the telemetry
preview, the AWS-facing collector authenticates the node and commits accepted
envelopes plus a reasoning job in one tenant-scoped transaction. Fleet
reasoning and alert delivery run in AWS; their failure is visible and does not
disable local status, doctor, evidence, cases, alerts, or the TUI.

For an isolated development deployment of the real collector and PostgreSQL:

```bash
export SYSPILOT_DEV_ADMIN_PASSWORD="$(openssl rand -hex 24)"
export SYSPILOT_DEV_RUNTIME_PASSWORD="$(openssl rand -hex 24)"
export SYSPILOT_DEV_PEPPER="$(openssl rand -hex 32)"
export SYSPILOT_DEV_TOKEN="spn_local.$(openssl rand -hex 24)"
docker compose -f compose.cloud-dev.yml up --build
```

These credentials are development-only, remain outside Git, and must never be
reused outside the loopback-only development stack.

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
    "bearer_credential": {
      "source": "environment",
      "variable": "SYSPILOT_TELEMETRY_TOKEN"
    },
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

Configuration schema v2 is stored at `~/.syspilot/config.json` and never contains
credential values. It contains credential-source metadata; owner-only secret files
default to `~/.syspilot/credentials/`. Do not commit either location.

Create an inspectable local support artifact with:

```bash
syspilot support bundle create
syspilot support bundle create ./syspilot-support.json
```

The JSON bundle is written owner-only and never uploaded automatically. Credential fields and configured sensitive context are redacted before the atomic write. Every component is marked `available`, `unavailable`, or `failed`; an incomplete bundle is still preserved for inspection but the command exits non-zero so partial collection cannot look successful.

| Field | Meaning |
|---|---|
| `active_provider` | `disabled`, `gemini`, `ollama`, or `syspilot`. |
| `gemini_credential`, `syspilot_credential` | Secret-free `CredentialRef` source metadata. Values resolve at runtime from environment, owner-only files, or systemd credentials. |
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
| AI request fails | Read the reported HTTP/provider message, then verify the selected provider, credentials, model, and endpoint. For Gemini 404 errors, run `syspilot model gemini-3.6-flash`. |
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
