# SysPilot feature and command reference

This document describes implemented behavior. SysPilot is a Linux operator tool: local diagnostics do not require AI, fleet enrollment, network access, or PostgreSQL.

## Command matrix

| Command | Purpose | Network | Persistent writes | Failure behavior |
|---|---|---:|---|---|
| `setup [--tui\|--line\|--check]` | Validated terminal wizard for local-only, collector, or hosted setup | Only for selected remote mode | Configuration and installation state after confirmation | Terminal fallback is explicit; cancellation is non-mutating; invalid choices or writes fail visibly. |
| `install [--binary [--force]]` | Install shell integration or user binary | No | User configuration/hooks/binary | Existing binary is preserved unless `--force`. |
| `uninstall [--binary]` | Remove hooks or user binary | No | Removes selected integration | Does not purge cases or configuration. |
| `status` | Summarize installation, provider, export, and daemon health | No | No | Missing/stale components are printed. |
| `doctor` | Probe platform, procfs, storage, configuration, export, AI, and daemon | No by default | Temporary storage probe | Returns non-zero for required unhealthy checks. |
| `evidence [--pid target]` | Capture deterministic system/process evidence | No | Owner-only case JSON | Missing targets and storage failures are fatal. |
| `cases list/show/export/delete` | Manage retained evidence cases | No | Export/delete when requested | Corrupt cases are reported and never skipped silently. |
| `alerts list/acknowledge/resolve/suppress` | Manage durable alert lifecycle | No | Atomic alert-state update | Invalid IDs/state files fail closed. |
| `support bundle create [path]` | Create inspectable redacted diagnostics | No | Atomic owner-only JSON | Partial artifact is retained and command exits `2`. |
| `daemon` | Run lifecycle ingestion, alert evaluation, health, and export | Collector only when enabled | Health, spool, alert state | Degraded Netlink/export state is recorded and logged. |
| `events` | Request daemon lifecycle events | Local socket | No | Connection, timeout, empty, and malformed responses are visible. |
| `monitor` | Interactive local process/TUI view | No, except optional AI action | Terminal only | Optional capability loss is displayed as degraded. |
| `provider/model/pull` | Configure or obtain AI models | `pull` and remote providers | Configuration/model data | Provider errors never replace offline diagnostics. |
| `index [--force]` | Build local code-context index | Embedding provider dependent | Local vector index | Invalid/corrupt index operations are reported. |
| `ask` | Optional AI explanation with file/index context | Provider dependent | May update index | AI failure is non-zero; no remediation runs. |
| `explain` | Gather telemetry and optionally causal/perf/eBPF context | Provider dependent | Optional reports/index | Evidence and inference are separated in the prompt. |
| `config telemetry ...` | Preview/enable/disable/show export | Preview is local | Atomic configuration | Export is opt-in; invalid endpoint/policy is rejected. |
| `config alert ...` | Add/remove/list deterministic rules | No | Atomic configuration | Duplicate/invalid rules are rejected. |
| `config set-key/set-url/set` | Change provider/settings | No | Atomic configuration | Configuration validates before commit. |
| `config rollback` | Restore immutable pre-migration bytes | No | Atomic restore plus preservation copy | Runs before normal config loading. |
| `fleet enroll/status/disable` | Explicit hosted-fleet lifecycle | Enrollment config only | Configuration/credential state | HTTPS and identity consistency are mandatory. |
| `completions bash/zsh/fish` | Generate completions | No | No | Unknown shells are rejected. |
| `version` | Print version | No | No | Independent of configuration. |

## Local evidence, cases, and alerts

`EvidenceBundleV1` contains a time range, node metadata, capability states, observations, findings, missing evidence, relationships, alert references, redaction record, and optional AI analysis. AI is never required to capture or read it. Cases default to 30 days or 1 GiB; retention deletion errors are fatal. Alert state is schema-versioned and persisted independently with firing, acknowledged, resolved, and suppressed states.

## Telemetry and fleet control

Export is disabled by default. Redaction occurs before durable spool persistence. The spool defaults to seven days or 512 MiB, uses monotonic node sequences, quarantines corrupt records, retries with jitter/server timing, and removes records only after valid acknowledgement. Agents send the public envelope to collectors; they never connect directly to the fleet PostgreSQL database. See [Telemetry protocol](TELEMETRY_PROTOCOL.md) and [Fleet database](FLEET_CONTROL_PLANE.md).

## Files and permissions

| Data | Default location | Mode/ownership |
|---|---|---|
| Configuration | `~/.syspilot/config.json` | User directory `0700`, file `0600` |
| Migration backups | Beside configuration | `0600`, never overwritten with different bytes |
| Evidence cases | `~/.syspilot/cases/*.json` | Directory `0700`, files `0600` |
| Support bundles | `~/.syspilot/support/*.json` | Directory `0700`, files `0600` |
| Alert state/spool | SysPilot data directory | Service/user-only |
| Runtime health/socket | `/run/syspilot` for packages; per-user runtime otherwise | Service policy |

Environment variables: `SYSPILOT_HOME` selects the data directory; `SYSPILOT_RUNTIME_DIR` selects runtime state; `GEMINI_API_KEY`, `SYSPILOT_API_KEY`, and `SYSPILOT_FLEET_TOKEN` provide runtime credentials. Support bundles redact persisted credentials and do not copy environment values.

## Exit semantics

`0` means the requested operation completed. `1` means validation, collection, encoding, storage, transport, or command usage failed. Support bundle creation uses `2` when a safe artifact was written but at least one requested component was unavailable or failed. No command automatically upgrades SysPilot, enrolls a fleet, uploads a support bundle, or executes AI-generated remediation.

## Machine output contract

`--json` emits `OutputEnvelopeV1` with schema version, command, generation timestamp, outcome, typed data, and diagnostics. The current stable commands are `status`, `doctor`, `evidence`, `cases list/show`, `alerts list/acknowledge/resolve/suppress`, `config telemetry show/preview`, `fleet enroll/status/disable`, `events`, and `support bundle create`. Mutation responses never contain bearer credentials. Outcomes map to exit codes `ok=0`, `error=1`, and `degraded=2`. Commands not yet migrated reject `--json` explicitly instead of emitting human text.
