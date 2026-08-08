# Daemon Reliability and Health

A process cannot report its own death. SysPilot therefore uses two independent layers:

1. The daemon atomically updates `/tmp/syspilot-health.json` every second. `syspilot status` reports it stale after three seconds.
2. The included [systemd service](../deploy/syspilotd.service) restarts the daemon after a crash.

Install the release binary and enable the service:

```bash
sudo install -m 0755 target/release/syspilot /usr/local/bin/syspilot
sudo install -m 0644 deploy/syspilotd.service /etc/systemd/system/syspilotd.service
sudo systemctl daemon-reload
sudo systemctl enable --now syspilotd
```

Verify both layers with `syspilot status`, `systemctl status syspilotd`, and `cat /tmp/syspilot-health.json`. Production host monitoring should alert on a heartbeat older than three seconds and failed service restarts.
