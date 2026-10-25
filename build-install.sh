#!/bin/sh
# Build the locked source tree and install the resulting user-local binary.
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$project_dir"

command -v cargo >/dev/null 2>&1 || {
    printf "%s\n" "SysPilot source installer: cargo is required." >&2
    exit 1
}

cargo build --release --locked
"$project_dir/target/release/syspilot" install --binary --force
printf "%s\n" "SysPilot is installed. Run: syspilot setup"
