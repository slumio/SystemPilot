# SysPilot documentation

This directory contains the current operational documentation for the Rust application built by Cargo.

## Current documentation

| Document | Purpose | Status |
|---|---|---|
| [Project README](../README.md) | Install, setup, AI, RCA, telemetry, alerts, configuration, troubleshooting. | Canonical user and operator guide. |
| [Architecture](../ARCHITECTURE.md) | Current Rust module boundaries, data flow, and delivery guarantees. | Canonical technical overview. |
| [Feature reference](FEATURES.md) | Every command family, offline/network behavior, persistence, permissions, and exit semantics. | Canonical product behavior reference. |
| [Engineering architecture](ENGINEERING.md) | SOLID boundaries, dependency direction, module ownership, and change rules. | Canonical maintainability guide. |
| [Testing](TESTING.md) | Local/CI gates and functional smoke matrix. | Canonical verification guide. |
| [Developer guide](../developer_guide.md) | Local development, tests, and module map. | Canonical developer guide. |
| [Contributing guide](../CONTRIBUTING.md) | Contribution policy and expectations. | Canonical contributor guide. |
| [Daemon reliability](RELIABILITY.md) | systemd service installation and health checks. | Canonical operations guide. |
| [Telemetry protocol](TELEMETRY_PROTOCOL.md) | Public schema-v1 collector and compatibility contract. | Canonical protocol specification. |
| [Fleet database](FLEET_CONTROL_PLANE.md) | PostgreSQL schema, tenant isolation, setup, ingestion transactions, and operations. | Canonical fleet-control storage guide. |
| [Fleet benchmark](FLEET_BENCHMARK.md) | Docker-based synthetic multi-agent transport load test and interpretation limits. | Development capacity tool; not a production fleet certification. |
| [Cloud operations](CLOUD_OPERATIONS.md) | AWS collector/reasoning deployment, secrets, health, scaling, and incident boundaries. | Operator guide for the commercial cloud foundation. |
| [Configuration migrations](CONFIG_MIGRATIONS.md) | Versioned upgrades, immutable backups, fail-closed validation, and rollback. | Canonical configuration recovery guide. |

## Documentation order

1. Read the project README for every user-facing setup path.
2. Read daemon reliability before running the daemon under systemd.
3. Read architecture and the developer guide before changing code.

The repository intentionally contains only current Rust implementation and operational documentation. Historical design exports, generated books, local build caches, and demo artifacts are not maintained here.
