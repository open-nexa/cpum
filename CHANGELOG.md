# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pushing a `v*` tag runs `.github/workflows/release.yml`, which publishes the
x64 and ARM64 NSIS installers, the signed updater artifacts, `latest.json` and
`SHA256SUMS.txt` in one GitHub release.

## [Unreleased]

### Added

- Everything needed to work on the project from a fork: `CONTRIBUTING.md`,
  `SECURITY.md`, `CODE_OF_CONDUCT.md`, issue templates, a pull request template,
  Dependabot and a `ci.yml` workflow that runs `cargo fmt --check`,
  `cargo clippy`, `cargo test`, `vue-tsc` and a production bundle on every pull
  request.
- CodeQL scanning (`codeql.yml`) and a `.gitleaks.toml` secret-scanning
  allowlist, so a leaked key fails the build instead of the release.
- This CHANGELOG.
- `.gitattributes`, pinning line endings so a checkout on another platform stops
  producing diffs that are only line endings.

### Changed

- The dynamic optimization (ProBalance) panel is now flagged as **Unstable** in
  the UI, both in the toolbar tooltip and as a chip next to the panel title, so
  it is clear the engine is still experimental.
- **Breaking for existing installations**: the bundle identifier moved from
  `com.eason.cpum` to `com.open-nexa.cpum`, which also moves the data directory
  from `%APPDATA%\com.eason.cpum` to `%APPDATA%\com.open-nexa.cpum`.
  `affinity_rules.json` is migrated automatically on first start; installers
  built before this change cannot be upgraded in place and must be uninstalled
  first.
- The updater endpoint now points at `open-nexa/cpum`. Installations of 0.1.1
  and older check `yixinin/cpum`, so they will not see this release; install
  0.2.0 once by hand to pick the new endpoint up.
- npm is the single supported package manager. `yarn.lock` is gone; use
  `npm ci` (CI) or `npm install` locally.
- The whole Rust tree is formatted with rustfmt, and CI now blocks on
  `cargo fmt --check` and on `cargo clippy -- -D warnings`.

### Fixed

- `rustls` updated to 0.23.45 in `src-tauri/Cargo.lock`, closing
  RUSTSEC-2026-0285 (TLS 1.3 handshake messages accepted across encryption
  level boundaries). It reaches the app through `tauri-plugin-updater`.

## [0.1.1] — 2026-09-18

### Fixed

- Saving a rule list could lose the edit that triggered the save.

## [0.1.0] — 2026-09-17

The first tagged release.

### Added

- CPU topology inspection: logical processors, physical cores, SMT threads,
  packages and CCD/die information, with Intel hybrid P/E-core distinction.
- A hierarchical process view with live CPU, memory, disk and network metrics.
- Per-process affinity editing on multi-processor-group systems (one mask per
  group rather than a single 64-bit value).
- Persistent affinity rules with exact / wildcard / path matching and a
  strict (`SetProcessAffinityMask`) or soft (`SetProcessDefaultCpuSets`) mode.
- Rules that also pin CPU priority class, I/O priority and memory priority.
- ProBalance: foreground-contention detection that downgrades background
  processes and restores them automatically, with status, statistics and a
  JSONL journal.
- `CpumAffinityService`, an optional LocalSystem Windows service that reapplies
  rules every five seconds, hosts the ProBalance runtime, and doubles as a
  privileged bridge over a named pipe so the un-elevated GUI never needs a UAC
  prompt to change a protected process.
- A per-user NSIS installer, a Tauri updater with minisign-signed artifacts, and
  SHA-256 checksums on every release.
