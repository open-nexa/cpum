# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pushing a `v*` tag runs `.github/workflows/release.yml`, which publishes the
x64 and ARM64 NSIS installers, the signed updater artifacts, `latest.json` and
`SHA256SUMS.txt` in one GitHub release.

## [Unreleased]

### Added

- Both READMEs now have an **Install** section — until now neither said where the
  installer comes from — covering the two architectures, the checksum check, the
  SmartScreen warning shown on unsigned installers, and the fact that 0.1.1 and
  older have to be upgraded by hand. A navigation line at the top of each README
  links the rest of the document.
- A download page at [open-nexa.github.io/cpum](https://open-nexa.github.io/cpum/),
  published from `docs/releases/` by `.github/workflows/pages.yml`. Links are read
  from the GitHub releases API and classified in `docs/releases/assets/releases.js`;
  nothing builds a filename from a template, so a renamed artifact cannot produce a
  404. There is a `localStorage` cache and a committed snapshot
  (`docs/releases/data/releases.json`, refreshed with `npm run snapshot:releases`)
  for when the API is unreachable or rate-limited. The page shows one installer per
  architecture: the pipeline publishes two (the canonical
  `CPU-Manager_<tag>_windows-<arch>-setup.exe` plus Tauri's own
  `CPU.Manager_<version>_<arch>-setup.exe`, which `latest.json` points at), and only
  the canonical one is offered to humans.
- `docs/ROADMAP.md`: what is solid, what is not, the next four phases, and a
  non-goals section with the reason for each one.
- `npm run lint:i18n` (`scripts/lint-i18n.mjs`), now part of CI. It fails when
  `zh-CN` and `en-US` in `src/i18n.ts` do not define the same key set, when a static
  `t('...')` key does not resolve, or when a CJK character appears anywhere outside
  `src/i18n.ts`. Zero dependencies; it parses the dictionary instead of
  regex-matching lines, because translation values contain `{}` placeholders.
- Screenshots in both READMEs at last: `processes-flat.png` (process list with the
  per-CCD affinity bars), `processes-tree.png`, `rules.png` and `probalance.png`,
  plus `screenshots/README.md` recording how they were captured so the next set
  matches.
- `.coderabbit.yaml`, configured as advisory only (no blocking review, no red
  commit status) with per-path instructions for the crates that have real
  constraints.

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
- Applying a rule now checks the current state first and skips the write when the
  process is already pinned / prioritised the way the rule wants. The service used
  to rewrite affinity and the three priority classes for every matching process
  every five seconds, and each of those writes makes the kernel re-evaluate thread
  placement for every thread of the process. A process whose state cannot be read
  is still written every time, so the change can only ever cost less, never stop a
  rule from being enforced. `cpum_service --apply-once --force` writes
  unconditionally when the skip logic needs to be ruled out.
- The per-second metrics round reuses the process handle it already holds to read
  the network counters, instead of resolving the PID back out of the handle and
  opening a second one. That was one extra `OpenProcess` per process per second,
  and it asked for `PROCESS_QUERY_INFORMATION`, which is denied more often than the
  limited-information handle the sampler falls back to - so the network column
  should now be populated for more processes, not just read faster.
- The per-process walk is single-threaded by default. At a few hundred processes
  the work is ~26 us of syscalls each, so the thread spawn costs more than it
  saves (measured: 11.3 ms with 8 threads vs 10.1 ms single-threaded); the
  parallel path now only engages above 600 processes.
- Two slow-changing fields are cached instead of re-read every round: the resolved
  exe path (keyed by PID *and* process creation time, so a recycled PID cannot be
  handed another process's path) and the three priority classes (re-read every
  2 s). A full refresh - the first frame, or a manual reload - still reads the
  priority classes for every process, so the values it shows are exact.
- The ProBalance panel no longer runs its own 2 s timer; `App.vue` calls it from
  the 1 s ticker it already runs, so the per-core usage poll and the panel refresh
  stop landing in the same frame.
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
