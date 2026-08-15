# Security policy

Do not disclose vulnerabilities in public issues. Report them privately to the
repository maintainers with affected versions, reproduction steps, and impact.

SysPilot can inspect process metadata and, when configured, send selected
context to external AI providers. Do not place credentials in issues, logs, or
sample configuration files.

## Credential entry

Use `syspilot config set-key <provider>`, `syspilot fleet enroll <endpoint> <node-id>`, and `syspilot config telemetry enable <endpoint> <node-id>` without a credential argument. Interactive use reads a hidden prompt; automation supplies stdin or a documented environment/systemd credential source. Never place credential values in command arguments, examples, logs, JSON output, or diagnostic reports.
