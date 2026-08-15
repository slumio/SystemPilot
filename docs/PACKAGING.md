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

Release automation builds static-musl `.deb` and RPM packages for x86_64 and
ARM64 with the same binary used in the signed release archive. Static linking
avoids coupling an artifact to the build runner's newer glibc. Both formats create a locked
`syspilot` service account, install the hardened systemd unit, and preserve
configuration, credentials, evidence, and state across upgrades and removal.

To build packages locally, install the pinned nfpm version used by CI and run:

```bash
rustup target add x86_64-unknown-linux-musl
# Debian/Ubuntu: sudo apt-get install musl-tools
cargo build --release --locked --target x86_64-unknown-linux-musl
export SYSPILOT_PACKAGE_ARCH=amd64
export SYSPILOT_PACKAGE_BINARY=target/x86_64-unknown-linux-musl/release/syspilot
export SYSPILOT_PACKAGE_VERSION=0.1.0
mkdir -p dist
deploy/packages/build.sh deb dist/syspilot.deb
SYSPILOT_PACKAGE_ARCH=x86_64 deploy/packages/build.sh rpm dist/syspilot.rpm
```

Packages never contain API keys. Place optional credentials through systemd's
credential store and use `syspilot doctor` to report availability without values.
Removing a package intentionally retains `/etc/syspilot`, `/var/lib/syspilot`,
and the service identity. An administrator must explicitly remove retained data.
