# Configuration migrations

SysPilot configuration is versioned independently from the binary. Schema version `1` is the current format.

## Upgrade behavior

When SysPilot reads an unversioned `config.json`, it performs this sequence before applying environment overrides:

1. Parse and validate the source JSON.
2. Write the original bytes to the immutable, owner-only `config.pre-v1.json` backup.
3. Apply the version-1 migration in memory.
4. Atomically replace `config.json`, sync it to disk, and restrict it to mode `0600`.
5. Parse and validate the migrated configuration before starting the requested command.

Migration errors stop the command with a non-zero exit. SysPilot never continues with defaults after a malformed, unsupported, or partially migrated configuration. Environment overrides remain runtime-only and are never copied into the migrated file or its backup.

A configuration whose schema is newer than the running binary is rejected. Upgrade SysPilot to a compatible version or explicitly restore the pre-migration file:

```console
syspilot config rollback
```

Rollback runs before ordinary configuration loading. It preserves the current file as immutable `config.pre-rollback-v1.json`, restores the exact pre-version-1 bytes atomically, and tells the operator to restart with the compatible binary. Existing backup files are never silently overwritten; conflicting contents cause rollback or migration to fail.

## Operational checks

- Keep `config.pre-v1.json` with the node data when preparing a binary rollback.
- Treat a migration or rollback error as an unavailable configuration state; inspect the named file and error before retrying.
- Do not edit backup files in place. Copy them elsewhere if manual recovery is required.
- A normal command after rollback may migrate the file again. Use rollback immediately before launching the older compatible binary.
