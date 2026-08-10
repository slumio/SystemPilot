# Packaging SysPilot

## Systemd service

The packaged daemon uses `/run/syspilot` for its socket and `/var/lib/syspilot`
for state. It runs as the dedicated `syspilot` user and group. Install the
binary as `/usr/bin/syspilot`, then install and enable the service:

```bash
sudo install -D -m 0644 deploy/syspilot.service /usr/lib/systemd/system/syspilot.service
sudo systemctl daemon-reload
sudo systemctl enable --now syspilot
```

Users who need the `events`, causal, or monitor connection to the system daemon
must belong to the `syspilot` group and start a new login session afterward.

## Release checklist

1. Confirm the MIT license remains appropriate for the release.
2. Update `CHANGELOG.md`, version in `Cargo.toml`, and create a signed `vX.Y.Z` tag.
3. Run `cargo fmt --all -- --check`, `cargo test --workspace`, and
   `cargo build --release --locked`.
4. Attach the CI-built binaries and SHA-256 checksums to the GitHub release.
5. Publish the workspace ABI crate first: `cargo publish -p syspilot-abi`.
6. Wait for crates.io to index it, then publish SysPilot: `cargo publish -p syspilot`.
7. Only then publish distribution-specific packages.

## Debian and RPM

The service and layout are ready for a `.deb`/RPM package. Before publishing one,
add distribution-specific maintainer scripts that create the `syspilot` system
user/group and preserve `/etc` configuration during upgrades. Do not package API
keys in a binary or a world-readable configuration file.
