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

cargo_root="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}"
installed_binary="${cargo_root}/bin/syspilot"
[ -x "$installed_binary" ] || fail "Cargo completed but ${installed_binary} was not created."

# Keep the user-local path used by `syspilot setup` in sync. Atomic replacement
# allows this to update an older binary even when a daemon is still executing it.
"$installed_binary" install --binary --force ||
    fail "the build succeeded, but the user-local binary could not be updated."

say ""
say "SysPilot installed successfully."
say "Binary: $HOME/.local/bin/syspilot"
say "Next: syspilot setup"
