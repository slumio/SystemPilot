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

## Release assets

The installer recognizes `syspilot-x86_64-unknown-linux-gnu.tar.gz` and
`syspilot-aarch64-unknown-linux-gnu.tar.gz`. Each archive must contain an
executable named `syspilot` and have a sibling `<archive>.sha256` asset whose
first field is the lowercase SHA-256 digest. Missing assets, unsupported architectures, download failures, and digest mismatches are hard failures and never alter or execute the installed binary.

Generate completion assets from the binary being packaged:

```bash
syspilot completions bash > syspilot.bash
syspilot completions zsh > _syspilot
syspilot completions fish > syspilot.fish
```

Install them in the distribution's standard locations, commonly
`/usr/share/bash-completion/completions/syspilot`,
`/usr/share/zsh/site-functions/_syspilot`, and
`/usr/share/fish/vendor_completions.d/syspilot.fish`.

Release automation publishes SPDX SBOMs, a checksum manifest, GitHub build provenance, and Sigstore bundles for both archives. Checksums detect corruption but do not authenticate a release by themselves; verify the signed tag and Sigstore bundle before installation when release authenticity is required.

## Release checklist

1. Confirm the MIT license remains appropriate for the release.
2. Update `CHANGELOG.md`, version in `Cargo.toml`, and create a signed `vX.Y.Z` tag.
3. Run `cargo fmt --all -- --check`, `cargo test --workspace`, and
   `cargo build --release --locked`.
4. Attach the two correctly named archives, per-archive SHA-256 files, SBOM,
   and checksum manifest to the GitHub release.
5. Publish the workspace ABI crate first: `cargo publish -p syspilot-abi`.
6. Wait for crates.io to index it, then publish SysPilot: `cargo publish -p syspilot`.
7. Only then publish distribution-specific packages.

## Debian and RPM

The service and layout are ready for a `.deb`/RPM package. Before publishing one,
add distribution-specific maintainer scripts that create the `syspilot` system
user/group and preserve `/etc` configuration during upgrades. Do not package API
keys in a binary or a world-readable configuration file.
