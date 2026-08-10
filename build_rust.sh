#!/usr/bin/env bash
set -euo pipefail

echo "🛠️  Compiling SysPilot — Rust Edition"
echo "     Allocator: mimalloc · Async: tokio · HTTP: reqwest · Netlink: nix/libc"

# Enable native CPU optimisations (AVX2, etc.) — mirrors the old -march=native flag
export RUSTFLAGS="-C target-cpu=native"

# Build in release mode
cargo build --release 2>&1

BINARY="./target/release/syspilot"

if [ -f "$BINARY" ]; then
    SIZE=$(du -sh "$BINARY" | cut -f1)
    echo ""
    echo "✅ syspilot built successfully — ${SIZE}"
    echo "   Binary: ${BINARY}"
    echo ""
    echo "Run:  ./target/release/syspilot --help"
else
    echo "❌ Build failed — binary not found."
    exit 1
fi
