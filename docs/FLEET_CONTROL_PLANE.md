# Fleet control-plane database

The fleet database belongs to the collector/control plane. SysPilot agents never connect to PostgreSQL and local `doctor`, evidence, TUI, alert, and case workflows remain operational without it.

> **Current capacity statement:** the repository now includes the first authenticated transactional HTTP collector, but it has not passed the PostgreSQL-backed production 1,000-server certification. Synthetic transport results remain development measurements, not a supported fleet maximum. See the [architecture scale boundary](../ARCHITECTURE.md#current-scale-boundary).

## Scope

Schema version 1 stores tenant-scoped principals and roles, node inventory and capabilities, hashed enrollment credentials, deduplicated telemetry envelopes, per-node sequence gaps, shared cases and annotations, alert lifecycle state, retention policy, deletion requests, and immutable audit events. Large evidence bundles belong in object storage; `cases.evidence_object_key` stores only the tenant-scoped reference.

PostgreSQL 14 or newer is required because sequence gaps use `int8multirange`. A dedicated database is recommended. The migration administrator must not be used by the running service.

## Apply the schema

Set an administrative PostgreSQL URI through the environment. TLS is required unless an operator explicitly opts into the isolated-development override.

```bash
export FLEET_DATABASE_URL='postgresql://migration-admin@db.example/syspilot?sslmode=verify-full'
deploy/fleet/setup-db.sh
```

The setup command verifies migration checksums and refuses edited migration history. It creates the non-login `syspilot_control_app` role. Create a separate login role through your secret manager and grant membership:

```sql
CREATE ROLE syspilot_runtime LOGIN PASSWORD 'secret-manager-generated-value';
GRANT syspilot_control_app TO syspilot_runtime;
```

Do not commit or place that password in agent configuration. Rotate the runtime credential independently of node enrollment credentials.

For an isolated local database only:

```bash
FLEET_DB_ALLOW_INSECURE=1 \
FLEET_DATABASE_URL='postgresql:///syspilot' \
deploy/fleet/setup-db.sh
```

## Mandatory request transaction

Every runtime transaction must set the authenticated tenant before accessing tenant data. Forced row-level security then rejects cross-tenant reads and writes.

```sql
BEGIN;
SET LOCAL syspilot.tenant_id = '00000000-0000-0000-0000-000000000001';
-- Execute one authorized request using parameterized statements.
COMMIT;
```

Use a transaction-scoped setting, never a session-scoped `SET`, because pooled connections are reused. The authorization layer must derive the tenant from the verified enrollment token or authenticated principal; it must never trust a tenant ID supplied only in a request body.

Ingestion must perform these actions in one transaction:

1. authenticate the node using a constant-time comparison against the stored token hash and reject expired or revoked credentials;
2. set `syspilot.tenant_id` locally;
3. insert each envelope using `(tenant_id, node_id, message_id)` plus its deterministic envelope digest; accept exact replays, reject changed content under the same ID, and use the sequence uniqueness constraint for conflicts;
4. lock and update `node_sequence_state`, recording gaps rather than hiding them;
5. update node `last_seen_at` and append an audit event when policy requires it;
6. commit before returning accepted IDs, highest accepted sequence, rejections, and retry timing.

The `syspilot-cloud` binary implements this boundary. It requires
`DATABASE_URL` for a login that is a member of `syspilot_control_app` and a
minimum 32-byte `SYSPILOT_CREDENTIAL_PEPPER`. Production injects both through
AWS Secrets Manager or workload identity; neither belongs in an image or
configuration file.

Committed envelopes create durable reasoning jobs. Run
`syspilot-reasoning-worker` under a login that is only a member of
`syspilot_cloud_worker`. Configure an HTTPS
`SYSPILOT_AWS_REASONING_ENDPOINT` and its secret-injected bearer credential.
Workers lease with `FOR UPDATE SKIP LOCKED`, recover expired leases after restarts, refuse credential-bearing redirects, retry bounded failures behind a short circuit, and store
object results transactionally. The endpoint receives the redacted envelope,
node ID, and message ID; it does not receive database credentials or tenant IDs.

A database error or failed commit acknowledges nothing. Never construct acknowledgements before the commit succeeds.

## Isolation and privilege verification

Run these checks using the runtime login before accepting traffic:

```sql
BEGIN;
SET LOCAL syspilot.tenant_id = '00000000-0000-0000-0000-000000000001';
SELECT count(*) FROM syspilot_control.nodes;
ROLLBACK;

SELECT * FROM syspilot_control.tenants;          -- must fail
SELECT * FROM syspilot_control.schema_migrations; -- must fail
UPDATE syspilot_control.audit_events SET action = 'changed'; -- must fail
```

Provision two test tenants and continuously test that neither can select, update, or delete the other's rows. Tenant IDs are part of every primary or foreign key that crosses a tenant-owned boundary.

## Operations

- Back up the database with encrypted, access-controlled PostgreSQL backups and test point-in-time recovery.
- Run retention and tenant deletion as a separate audited administrative worker. The runtime role cannot mutate audit history.
- Alert on migration checksum mismatch, connection-pool exhaustion, ingestion rollback, deduplication conflicts, sequence gaps, RLS denial, retention lag, and deletion failure.
- Use bounded statement and lock timeouts. Do not retry non-idempotent transactions without an idempotency key.
- Store raw enrollment tokens only at issuance time. Persist only a cryptographic hash and a non-secret prefix for operator identification.
- Treat database health as fleet-control health. It must never change local agent health into a failure.

## Files

- `deploy/fleet/postgres/001_initial.sql`: transactional schema and security migration.
- `deploy/fleet/setup-db.sh`: TLS-enforcing, checksum-aware migration runner.
- `docs/TELEMETRY_PROTOCOL.md`: public agent/collector acknowledgement contract.
