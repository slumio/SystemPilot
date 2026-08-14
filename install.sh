#!/bin/sh
set -eu

REPOSITORY="slumio/SystemPilot"
REF="${SYSPILOT_REF:-dev}"

say() { printf '%s\n' "$*"; }
fail() { printf 'SysPilot installer: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || fail "Linux is required."
for command_name in curl tar cargo; do
    command -v "$command_name" >/dev/null 2>&1 || fail "'$command_name' is required. Install Rust from https://rustup.rs, then retry."
done

install_dir=$(mktemp -d "${TMPDIR:-/tmp}/syspilot-install.XXXXXX") || fail "could not create a temporary directory."
trap 'rm -rf "$install_dir"' EXIT HUP INT TERM

archive_url="https://github.com/${REPOSITORY}/archive/${REF}.tar.gz"
archive_path="${install_dir}/source.tar.gz"
say "Downloading SysPilot (${REF})..."
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
    --output "$archive_path" "$archive_url" ||
    fail "could not download ref '${REF}'. Use an existing branch or release tag."
tar -xzf "$archive_path" --strip-components=1 -C "$install_dir" ||
    fail "the downloaded source archive is invalid."
rm -f "$archive_path"

say "Building and installing SysPilot..."
cargo install --locked --path "$install_dir"

if command -v syspilot >/dev/null 2>&1; then
    say ""; say "SysPilot installed successfully."; say "Next: syspilot setup"
else
    cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
    say ""; say "SysPilot installed to ${cargo_bin}/syspilot."; say "Add ${cargo_bin} to PATH, then run: syspilot setup"
fi
