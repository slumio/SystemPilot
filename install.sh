#!/bin/sh
set -eu

REPOSITORY="slumio/SystemPilot"
VERSION="${SYSPILOT_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
fail() { printf 'SysPilot installer: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || fail "Linux is required."
for command_name in curl tar sha256sum awk find install; do
    command -v "$command_name" >/dev/null 2>&1 || fail "'$command_name' is required."
done

install_dir=$(mktemp -d "${TMPDIR:-/tmp}/syspilot-install.XXXXXX") || fail "could not create a temporary directory."
trap 'rm -rf "$install_dir"' EXIT HUP INT TERM

architecture=$(uname -m)
case "$architecture" in
    x86_64) release_target="x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) release_target="aarch64-unknown-linux-gnu" ;;
    *) release_target="" ;;
esac

install_release() {
    [ -n "$release_target" ] || fail "unsupported architecture: $architecture (supported: x86_64, aarch64)"
    asset="syspilot-${release_target}.tar.gz"
    if [ "$VERSION" = "latest" ]; then
        release_base="https://github.com/${REPOSITORY}/releases/latest/download"
    else
        release_base="https://github.com/${REPOSITORY}/releases/download/${VERSION}"
    fi
    say "Trying checksum-verified SysPilot release (${VERSION}, ${release_target})..."
    curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --output "${install_dir}/${asset}" "${release_base}/${asset}" || fail "could not download release archive: ${release_base}/${asset}"
    curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --output "${install_dir}/${asset}.sha256" "${release_base}/${asset}.sha256" || fail "could not download release checksum: ${release_base}/${asset}.sha256"
    expected=$(awk 'NR == 1 { print $1 }' "${install_dir}/${asset}.sha256")
    actual=$(sha256sum "${install_dir}/${asset}" | awk '{ print $1 }')
    [ -n "$expected" ] && [ "$actual" = "$expected" ] || fail "release checksum verification failed"
    mkdir -p "${install_dir}/release" "$HOME/.local/bin" || fail "could not create installation directories"
    tar -xzf "${install_dir}/${asset}" -C "${install_dir}/release" || fail "checksum-verified release archive is invalid"
    release_binary=$(find "${install_dir}/release" -type f -name syspilot -print -quit)
    [ -n "$release_binary" ] && [ -f "$release_binary" ] || fail "checksum-verified release archive does not contain syspilot"
    install -m 0755 "$release_binary" "${HOME}/.local/bin/.syspilot-install" || fail "could not stage the release binary"
    mv "${HOME}/.local/bin/.syspilot-install" "${HOME}/.local/bin/syspilot" || fail "could not atomically replace the installed binary"
}

install_release
say "Next: syspilot setup"
