# SysPilot roadmap

SysPilot is building an evidence-first Linux diagnostics tool: collect observable
signals locally, make the causal chain inspectable, and let users choose where
AI reasoning runs. The roadmap is deliberately concrete so contributors can
select bounded work.

## Local 1.0 implemented reliability baseline

- Configuration schema v2 persists only `CredentialRef` metadata. Secrets resolve
  from owner-only files, environment variables, or systemd credentials; v0/v1
  migration preserves an owner-only rollback backup and commits credentials first.
- The public command grammar is validated by typed Clap commands with a global
  `--json` flag. Existing command spellings remain supported and ambiguous setup
  modes are rejected.
- Causal analysis and the monitor use the typed daemon client and emit structured
  degraded diagnostics before bounded procfs fallback. Procfs fallback collection
  is shared through a short-lived `ProcSnapshot` cache.
- Vector indexes distinguish missing, loaded, and corrupt state. Corrupt evidence
  is quarantined and requires an explicit forced rebuild.
- The monitor uses Ratatui and Crossterm for drawing, input, resize events, and
  RAII terminal restoration. Restricted terminals fail with an actionable fallback.
- Causal HTML reports are self-contained, script-safe, and require no CDN.
  Configuration, report, index, spool, evidence, and support writes report failures.

## Remaining hosted-control-plane work

- Harden the systemd package path: Debian/RPM maintainer scripts, a dedicated
  service user/group, and upgrade-safe configuration handling.
- Add deterministic tests for local Ollama/Qwen embedding retrieval and index
  invalidation when the embedding model changes.
- Improve provider errors with retry hints, quota details, and safe diagnostics.
- Expand the future self-hosted HTTP collector independently from the Linux
  kernel-ingress crate. The latter is a local event transport, not an HTTP server.

## Diagnostics and observability

- Expand process-causality evidence while clearly separating observation from
  inference.
- Improve capability detection for kernel stacks, perf, eBPF, and restricted
  containers.
- Make daemon health, Netlink availability, and event drops visible in `status`.

## Distribution and trust

- Publish `syspilot-abi` followed by `syspilot` to crates.io.
- Ship signed Linux x86_64 and ARM64 release binaries with checksums.
- Add `.deb`, RPM, Homebrew, and AUR packaging after the system-service layout
  has been tested on supported distributions.
- Produce SBOMs and reproducible-build documentation.

## How to contribute

1. Pick a small item above or inspect module-level TODOs.
2. Open an issue describing the intended behavior before a large design change.
3. Add tests for observable behavior and run the strict checks from
   [CONTRIBUTING.md](../CONTRIBUTING.md).
4. Keep claims about root cause calibrated to the evidence available.

Good first contributions include documentation corrections, tests for procfs
edge cases, provider error parsing, shell completions, and packaging scripts.
