#!/bin/bash
set -e

echo "🛠  Building SysPilot via Xmake — High-Performance System Intelligence Suite"
echo "     Libraries (xrepo): mimalloc · simdjson · spdlog · fmt · TBB · libcurl"
echo "     Vendored headers: tsl::robin_map · ConcurrentQueue"

# Ensure local user path is searched for xmake/xrepo
export PATH="$PATH:$HOME/.local/bin"

if ! command -v xmake &> /dev/null; then
    echo "❌ error: xmake command not found. Please install Xmake (https://xmake.io) first."
    exit 1
fi

# Configure release build
xmake f -m release -y

# Build target
xmake -y

# Extract generated target binary path and copy to project root
TARGET_PATH=$(xmake show --format=json -t syspilot | grep '"targetfile":' | cut -d'"' -f4 | sed 's/\\//g')
if [ -f "$TARGET_PATH" ]; then
    cp "$TARGET_PATH" ./syspilot
    echo "✅  syspilot built successfully and copied to root directory."
else
    echo "❌ error: Built target binary not found at '$TARGET_PATH'"
    exit 1
fi

echo ""
echo "  Run:  ./syspilot daemon &   # start background telemetry daemon"
echo "        ./syspilot monitor    # launch live TUI"
