#!/bin/sh
set -eu

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi

# Configuration, credentials, evidence, and the service identity deliberately
# survive removal so reinstall and rollback are lossless. Purging them is an
# explicit administrator action.
exit 0
