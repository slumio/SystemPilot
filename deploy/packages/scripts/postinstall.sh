#!/bin/sh
set -eu

if ! getent group syspilot >/dev/null 2>&1; then
    groupadd --system syspilot
fi

if ! id syspilot >/dev/null 2>&1; then
    useradd --system --gid syspilot --home-dir /var/lib/syspilot \
        --no-create-home --shell /usr/sbin/nologin syspilot
fi

install -d -o root -g syspilot -m 0750 /etc/syspilot
install -d -o syspilot -g syspilot -m 0750 /var/lib/syspilot

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    if [ -d /run/systemd/system ]; then
        systemctl enable syspilot.service >/dev/null
        systemctl try-restart syspilot.service >/dev/null 2>&1 || \
            systemctl start syspilot.service >/dev/null
    fi
fi
