# Verification and release testing

## Required local gates

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo build --release --locked
sh -n install.sh deploy/fleet/setup-db.sh
git diff --check
```

Unit tests cover parsing, schemas, redaction, acknowledgement validation, retry outcomes, spool recovery, monotonic sequences, alert lifecycle, configuration migration/rollback, support sanitization, case retention, causal graphs, profiling, safety rules, streaming, and collector queues. Integration tests cover public library behavior and live Linux procfs where appropriate.

The `fleet-transport-benchmark` CI job additionally runs the Docker-based synthetic 1,000-server telemetry gate. `fleet-peak-benchmark` removes pacing and enforces a minimum saturated envelope rate plus failure and p95 latency limits. See [Fleet transport benchmark](FLEET_BENCHMARK.md) for thresholds and interpretation limits.

## Functional smoke matrix

| Area | Success check | Failure/degraded check |
|---|---|---|
| Configuration | Save/load secret-free schema 2 and resolve credential sources | Malformed/newer schema, permissive credentials, and interrupted migration rejected; rollback still runs |
| Evidence | Capture, persist, list, show/export | Missing PID and corrupt case visible |
| Support | Credential-free `0600` artifact | Missing/malformed component produces exit `2` |
| Daemon | Socket response and fresh health | Missing/stale health and Netlink degradation visible |
| Telemetry | Preview, spool, valid ack removal | Bad ack/rejection/retry/quarantine visible |
| Alerts | Fire, acknowledge, resolve, suppress | Corrupt state rejected |
| Fleet | HTTPS enrollment/status/disable | Invalid endpoint/identity rejected |
| AI | Each configured provider streams output | HTTP/payload errors return non-zero |
| Packaging | Install/upgrade/uninstall artifact | Signature/checksum mismatch blocks install |

CI additionally applies the PostgreSQL schema twice, tests tenant RLS, denies global-table runtime access, and verifies audit immutability. Release promotion also requires artifact checksum/signature/SBOM/provenance checks and supported-distribution installation tests.
