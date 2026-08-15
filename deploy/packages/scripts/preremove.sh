#!/bin/sh
set -eu

# Debian passes "remove"/"purge"; RPM passes 0 for the final erase. Upgrades
# intentionally leave the running service and all state intact.
case "${1:-}" in
    remove|purge|0)
        if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
            systemctl disable --now syspilot.service >/dev/null 2>&1 || true
        fi
        ;;
esac
