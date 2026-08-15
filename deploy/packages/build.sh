#!/bin/sh
set -eu

packager=${1:?usage: build.sh deb|rpm OUTPUT}
output=${2:?usage: build.sh deb|rpm OUTPUT}
: "${SYSPILOT_PACKAGE_ARCH:?}"
: "${SYSPILOT_PACKAGE_BINARY:?}"
: "${SYSPILOT_PACKAGE_VERSION:?}"

case "$packager" in deb|rpm) ;; *) printf 'unsupported packager: %s\n' "$packager" >&2; exit 2 ;; esac
case "$SYSPILOT_PACKAGE_ARCH" in ''|*[!A-Za-z0-9_-]*) printf 'invalid package architecture\n' >&2; exit 2 ;; esac
case "$SYSPILOT_PACKAGE_VERSION" in ''|*[!A-Za-z0-9.+~-]*) printf 'invalid package version\n' >&2; exit 2 ;; esac
case "$SYSPILOT_PACKAGE_BINARY" in ''|*[!A-Za-z0-9_./-]*) printf 'invalid binary path\n' >&2; exit 2 ;; esac
test -x "$SYSPILOT_PACKAGE_BINARY" || { printf 'package binary is not executable: %s\n' "$SYSPILOT_PACKAGE_BINARY" >&2; exit 1; }
command -v nfpm >/dev/null 2>&1 || { printf 'nfpm is required\n' >&2; exit 1; }

rendered=$(mktemp "${TMPDIR:-/tmp}/syspilot-nfpm.XXXXXX")
trap 'rm -f "$rendered"' EXIT HUP INT TERM
sed -e "s|@ARCH@|$SYSPILOT_PACKAGE_ARCH|g" \
    -e "s|@VERSION@|$SYSPILOT_PACKAGE_VERSION|g" \
    -e "s|@BINARY@|$SYSPILOT_PACKAGE_BINARY|g" \
    deploy/packages/nfpm.yaml >"$rendered"
nfpm package --config "$rendered" --packager "$packager" --target "$output"
