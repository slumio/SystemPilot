# Fleet transport benchmark

This benchmark answers a narrow question: how many simulated agents can concurrently send SysPilot-compatible batches to a stateless acknowledgement endpoint on the tested Docker host?

It does **not** certify production fleet capacity. The synthetic collector deliberately excludes enrollment authentication, PostgreSQL transactions, row-level security, reasoning, retention, object storage, and replay storms. The real collector now exists, but production capacity must be measured separately with its PostgreSQL and AWS dependencies enabled.

## Run

```bash
docker compose -f compose.fleet-benchmark.yml up --build --abort-on-container-exit --exit-code-from load
```

The default run simulates 1,000 servers, one request per server per second, 16 envelopes per request, for 30 seconds. The load container exits unsuccessfully when failures exceed 0.1% or approximate p95 latency exceeds 500 ms.

## CI gate

The `fleet-transport-benchmark` job in `.github/workflows/ci.yml` runs the same 1,000-server scenario for every push to `dev` or `master` and for every pull request. The separate `fleet-peak-benchmark` job runs 16 continuously active senders with 1,024-record batches and requires at least 750,000 envelopes/second, no more than 0.1% failures, and p95 latency no greater than 100 ms. Both jobs upload their JSON report even after failure, so regressions remain inspectable.

This gate intentionally uses a fixed synthetic workload. Increase the published capacity only through a reviewed change that includes the new result and runner specification; do not silently weaken latency or failure thresholds.

Scale one dimension at a time:

```bash
VIRTUAL_SERVERS=5000 DURATION_SECONDS=120 \
docker compose -f compose.fleet-benchmark.yml up --build --abort-on-container-exit --exit-code-from load
```

Run continuously in-flight saturation mode:

```bash
LOAD_MODE=saturation VIRTUAL_SERVERS=16 BATCH_SIZE=1024 \
DURATION_SECONDS=30 MIN_ENVELOPES_PER_SECOND=750000 MAX_P95_MS=100 \
docker compose -f compose.fleet-benchmark.yml up --build --abort-on-container-exit --exit-code-from load
```

Always remove the completed benchmark stack:

```bash
docker compose -f compose.fleet-benchmark.yml down --volumes --remove-orphans
```

## Interpret the report

The load process prints one JSON document containing successful and failed requests, request and envelope throughput, approximate latency bounds, and whether thresholds passed. Record the CPU architecture, Docker version, assigned CPU/memory, batch size, duration, and complete JSON result with every published number.

A successful 5,000-server synthetic run means only that the stateless HTTP protocol path handled that workload under those resource limits. It does not mean the PostgreSQL-backed production control plane supports 5,000 servers.

### Current development baseline

On an 8-thread Intel Core i5-1135G7 development machine, a direct (non-Docker) 1,000-server, 15-second run produced 15,000 successful requests, no failed requests, approximately 998 requests/second and 15,971 envelopes/second, with p95/p99 latency in the 50 ms-or-lower histogram bucket. This is a development baseline, not a production fleet certification; CI artifacts are the repeatable comparison record.

The same machine's 10-second saturation sweep peaked at approximately 1,224,387 envelopes/second using 16 continuously active senders and 1,024-record batches. It completed 11,968 acknowledged requests with zero failures, p95 no greater than 20 ms, and p99 no greater than 50 ms. For comparison, 256-record batches peaked at approximately 1,160,988 envelopes/second. The configured 10,000-record maximum was slower at approximately 818,568 envelopes/second and used about 214 MiB of load-generator resident memory, demonstrating that the largest batch is not the fastest or most memory-efficient setting.

These figures include JSON request generation and acknowledgement parsing. They still exclude authentication and durable database work.

## Required production scenarios

Before publishing a fleet limit, benchmark the real collector with:

- valid, expired, revoked, and cross-tenant credentials;
- PostgreSQL row-level security and realistic connection-pool limits;
- steady lifecycle traffic and bursty alert traffic;
- duplicate messages, sequence gaps, partial acknowledgement, and malformed input;
- collector restart, database slowdown, network loss, and simultaneous spool replay;
- retention workers, audit writes, backups, and replicas enabled;
- at least one hour of steady state and a longer soak at the intended limit.
