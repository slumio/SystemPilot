# Developer Guide

## Setup

Install the stable Rust toolchain and build from the repository root:

```bash
cargo build --release
cargo test --workspace
```

Use `cargo build --release` for a release build. The executable is `target/release/syspilot`.

## Code map

The authoritative responsibility and dependency rules are in [Engineering architecture](docs/ENGINEERING.md). New code must keep process exits and rendering at the CLI boundary, return typed failures from adapters, and avoid introducing AI, fleet, or database dependencies into local diagnostics.

Start with `src/main.rs` for CLI behavior and `src/lib.rs` for library exports. Core functionality is split into focused Rust modules:

- `daemon.rs`: Netlink lifecycle events and UNIX socket server.
- `telemetry.rs`: procfs readers and telemetry serialization.
- `causal_engine.rs`: resource/process graph and causal traversal.
- `ai.rs`: streaming AI-provider requests.
- `codebase.rs`: source chunking and vector index.
- `ui/`: terminal UI and stream rendering.

Shared telemetry types live in `crates/syspilot-abi`; bounded collection behavior lives in `crates/syspilot-collector`.

## Development workflow

Format and test before submitting changes:

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
```

Run a local smoke test with:

```bash
./target/release/syspilot --help
./target/release/syspilot explain --pid $$ --causal
```

The daemon, procfs telemetry, tracing, and profiling are Linux-specific. Test failure paths as well as normal process snapshots when changing these modules.
