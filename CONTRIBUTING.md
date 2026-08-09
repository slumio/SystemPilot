# Contributing to SysPilot

SysPilot is a Rust 2021 Linux diagnostics project. Keep changes focused, covered by tests, and compatible with the supported stable Rust toolchain.

## Development

```bash
cargo build
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

Use `./build_rust.sh` when you need the locally optimised release binary. It writes the executable to `target/release/syspilot`.

## Code conventions

- Follow `rustfmt`; do not hand-format around it.
- Prefer explicit error handling and preserve useful OS error context.
- Keep Linux-specific calls isolated in the module that owns them.
- Avoid unbounded queues or allocations on the daemon event path.
- Update integration tests for behavioral changes to telemetry, causal analysis, safety, or streaming output.

## Pull requests

Describe the behavior change, its Linux/runtime assumptions, and how you verified it. Include test output or a concise manual test plan for changes that interact with Netlink, procfs, the terminal UI, or external AI providers.
