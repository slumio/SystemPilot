# SysPilot documentation

This directory contains the current operational documentation for the Rust application built by Cargo.

## Current documentation

| Document | Purpose | Status |
|---|---|---|
| [Project README](../README.md) | Install, setup, AI, RCA, telemetry, alerts, configuration, troubleshooting. | Canonical user and operator guide. |
| [Architecture](../ARCHITECTURE.md) | Current Rust module boundaries, data flow, and delivery guarantees. | Canonical technical overview. |
| [Developer guide](../developer_guide.md) | Local development, tests, and module map. | Canonical developer guide. |
| [Contributing guide](../CONTRIBUTING.md) | Contribution policy and expectations. | Canonical contributor guide. |
| [Daemon reliability](RELIABILITY.md) | systemd service installation and health checks. | Canonical operations guide. |

## Documentation order

1. Read the project README for every user-facing setup path.
2. Read daemon reliability before running the daemon under systemd.
3. Read architecture and the developer guide before changing code.

The repository intentionally contains only current Rust implementation and operational documentation. Historical design exports, generated books, local build caches, and demo artifacts are not maintained here.
