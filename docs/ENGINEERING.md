# Engineering architecture and SOLID rules

## Dependency direction

```text
CLI/TUI presentation
        |
application services (doctor, evidence, alerts, fleet, support)
        |
domain contracts (config, protocol schemas, findings, lifecycle state)
        |
adapters (procfs, Netlink, Unix socket, filesystem spool, HTTP collector)
```

Dependencies point inward. Domain records do not print, exit the process, initialize AI, or select storage paths. Presentation converts typed errors to operator messages. Adapters expose failure instead of manufacturing domain defaults.

## SOLID application

- Single responsibility: configuration migration, JSON presentation, evidence retention, support sanitization, alert state, spool delivery, and fleet enrollment have separate modules.
- Open/closed: version fields and typed envelopes allow new schema handlers without changing old records in place.
- Liskov substitution: collectors implement the public acknowledgement contract; full-batch HTTP success remains an explicitly documented schema-v1 behavior.
- Interface segregation: local evidence does not depend on AI or fleet interfaces; the agent never receives a database interface.
- Dependency inversion: CLI/TUI consume domain/application functions; filesystem, procfs, HTTP, Netlink, and terminal details remain boundary concerns.

## Module ownership

| Module | Responsibility | Must not own |
|---|---|---|
| `config`, `config_migration` | Schema validation, atomic migration/rollback | CLI rendering, network probing |
| `output` | Typed JSON encoding/decoding for presentation | Domain policy |
| `telemetry` | procfs/system collection | Fleet delivery or AI |
| `evidence` | Evidence schema, capture, case retention | Hosted persistence |
| `alert` | Durable lifecycle transitions | AI triggering/remediation |
| `distributed`, `spool` | Envelope/redaction, acknowledgement, replay | PostgreSQL |
| `fleet` | Enrollment consistency and operator status | Direct database access |
| `support` | Mandatory sanitization and atomic artifact creation | Upload |
| `daemon` | Runtime orchestration/transports | CLI configuration editing |
| `daemon_client` | Typed local-socket requests, timeouts, and response validation | Rendering or silent procfs fallback |
| `daemon_health` | Typed health-file decoding and freshness calculation | Rendering or zero-valued error defaults |
| `ai`, `codebase` | Optional explanation and context | Diagnostics availability |
| `ui` | Terminal state/rendering | Persistent domain rules |

## Error policy

Production input, filesystem, terminal, serialization, protocol, and response errors use typed results or explicit degraded component records. Defaults are allowed only for documented absence/default configuration, never to conceal corrupt or malformed state. Unsafe Linux calls require a nearby invariant and a focused test or capability check. Process termination belongs at the binary boundary.

## Change rules

Schema changes require a version, compatibility fixture, migration note, and rollback story. New export data must be redacted before persistence and previewable. New fleet storage keys and authorization decisions must include tenant scope. New commands require help text, completions, documentation, invalid-usage tests, and non-zero failure behavior.
