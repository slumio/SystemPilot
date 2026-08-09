# SysPilot documentation

This directory contains current operational documentation and historical design material. Read the current documents first; they describe the Rust application built by Cargo.

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

## Historical material

The large textbooks, architecture books, observability reports, and generated HTML/PDF books in this directory retain C++-era material. They are historical reference only. They are not current build instructions, API contracts, deployment guidance, or a statement of current behavior.

If historical material conflicts with a current document, the current document is authoritative. Do not use historical C++ examples, dependencies, build commands, benchmarks, latency claims, or deployment instructions for the Rust application.
