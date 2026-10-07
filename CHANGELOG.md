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

- **Dependencies, in one branch instead of 16 dependabot pull requests.** The
  frontend moves to Vite 8.3, `@vitejs/plugin-vue` 6.0, `vue-tsc` 3.3, Vuetify
  4.2, `sass-embedded` 1.105 and TypeScript 6.0; the Rust side to `tauri` 2.12,
  `tauri-build` 2.7, `tauri-plugin-opener` 2.7, `tauri-plugin-updater` 2.13,
  `uuid` 1.27 and `windows-service` 0.8. The GitHub Actions group is bumped
  across all four workflows.
  - **TypeScript stops at 6.0, not 7.0.** Dependabot proposes 7.0.2, which cannot
    work here: that release is the native (Go) compiler and its `exports` map has no
    `./lib/tsc`, while `vue-tsc` — the only way `vue-tsc --noEmit` type-checks the
    SFCs — resolves `typescript/lib/tsc` and dies with
    `ERR_PACKAGE_PATH_NOT_EXPORTED`. 6.0.3 is the last release before that rewrite
    and still ships `lib/tsc`. Revisit when `vue-tsc` supports the native compiler.
  - The Rust and JavaScript halves of each Tauri plugin are pinned to the same
    version (`tauri-plugin-opener` 2.7.0, `tauri-plugin-updater` 2.13.1,
    `@tauri-apps/cli` 2.12.1). `tauri build` refuses to bundle when they differ on
    major/minor, which it reported for the versions dependabot proposed on the two
    sides separately.
  - `actions/dependency-review-action` is pinned to `v5.0.0` instead of the `@v5`
    dependabot asks for: that repository publishes no floating `v5` tag, so `@v5`
    does not resolve.
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
- The process table is virtual-scrolled. `v-data-table` ran with
  `items-per-page="-1"`, so every row was live in the DOM and every metrics tick
  patched all ~390 of them even though only ~20 are on screen; the new
  `src/components/ProcessTable.vue` renders the viewport plus a small overscan
  instead. Sorting moved into the component (one column at a time; CPU, memory
  and priority start descending). Columns, tree indentation, right-click menu,
  priority colours and the per-CCD affinity swatches are unchanged, and the
  tree-mode child count is now built once per list change instead of scanning
  the process list once per row.
- npm is the single supported package manager. `yarn.lock` is gone; use
  `npm ci` (CI) or `npm install` locally.
- The whole Rust tree is formatted with rustfmt, and CI now blocks on
  `cargo fmt --check` and on `cargo clippy -- -D warnings`.
- `vue` 3.5.41 -> 3.5.43 (`@vue/server-renderer` XSS, GHSA-g2v6-rqmx-r4w6) and
  `source-map-js` 1.2.1 -> 1.2.2 (event-loop denial of service,
  GHSA-68fv-2mgg-jv7q). Both are flagged by `npm audit --omit=dev
  --audit-level=high`, which CI runs; neither path is reachable from this app,
  but the audit job fails on them.
- `docs/PERFORMANCE-FIX-PLAN.md` linked to `PERFORMANCE.md` as if the two were
  siblings in `docs/`. The audit is not committed - it is a working document
  kept in `.workbuddy/`, because its numbers are re-measured per machine - so
  the reference is now plain text and says where the audit actually lives.
- The `dependency review` check no longer fails every pull request. The action
  reads the repository's dependency graph, which has to be enabled in the
  repository settings - something a pull request cannot do - and while it is off
  the action always exits 1 with "Dependency review is not supported on this
  repository" whatever the code does. That failure is now tolerated until the
  repository variable `DEPENDENCY_REVIEW_ENABLED` is `true`; set it once the
  setting is on and the job blocks again. Its severity threshold is `high`, the
  same as the `npm audit` step in the same workflow.

### Fixed

- Four CSS rules in `App.vue` never applied. The non-scoped `<style>` block used
  `:deep(.v-main)`, `:deep(.v-container)`, `:deep(.v-data-table table)` and
  `:deep(.v-data-table th/td)`, but Vue only rewrites `:deep()` inside a *scoped*
  block — elsewhere it is passed through untouched, and a `:deep(...)` selector
  matches nothing. So the viewport height constraint on the main layout and the
  fixed-column / ellipsis rules for the process table were all inert, and had been
  since those rules were written. Vite 6's esbuild minifier dropped them silently;
  Vite 8 switched to lightningcss, which reports "'deep' is not recognized as a
  valid pseudo-class" and so made the dead rules visible. The two layout rules are
  now plain global selectors, which is what the non-scoped block was for. The two
  table rules are gone rather than rewritten: the virtual-scrolled process table
  (`ProcessTable.vue`) no longer uses `v-data-table` and implements the fixed column
  widths and the ellipsis itself.
- `rustls` updated to 0.23.45 in `src-tauri/Cargo.lock`, closing
  RUSTSEC-2026-0285 (TLS 1.3 handshake messages accepted across encryption
  level boundaries). It reaches the app through `tauri-plugin-updater`.
- Applying rules twice could under-report the "applied to N processes" count.
  The GUI's own pass counted the processes it found already correct, but when
  the elevated service had to do the work instead, those processes were lost:
  the service returns them as "skipped", not as writes, so they never appeared
  in `changed`. `ApplyReport` and the IPC response now carry `skipped_pids`,
  and both passes feed one de-duplicating set, so a process is counted exactly
  once no matter which pass saw it.
- `get_process_group_masks` could report a process as already matching a rule
  when only some of its threads had been read. A process's affinity is the
  union over its threads, so a mask built from half of them can equal the
  rule's masks while the unobserved threads run somewhere else - which made the
  idempotent apply skip a write that was still needed. The read is now
  all-or-nothing: if any thread cannot be opened, or a thread reports a
  processor group this process does not know about, the function returns `None`
  and the caller writes.
- The NSIS bundle step in CI never got far enough to fail for a real reason. It
  passed its updater override as inline JSON escaped with backslashes, which is
  right for bash but not for PowerShell, the default shell on `windows-latest`:
  the CLI received literal backslashes and rejected the value. The override is
  now a file, `src-tauri/tauri.no-updater.conf.json`, so no shell is involved.
  The release workflow's unsigned branch had the same bug and uses the same
  file.
- The process table sorted its rows globally even in tree mode, so a child with
  a higher CPU figure than its parent moved away from it while keeping the
  parent's indentation - expanding the parent no longer revealed its children.
  Rows are now ordered within each sibling group instead, so every node keeps
  its own subtree directly beneath it.
- The process table clipped its right-hand columns in a narrow window with no
  way to reach them. The columns have minimum widths and the body now scrolls
  horizontally, with the header following the rows.
- The 2-second priority cache was keyed on PID alone, so a recycled PID could
  be served its predecessor's priority classes until the next refresh wave -
  `prune_caches` only drops PIDs that have disappeared, and a recycled PID has
  not. It now stores the process creation time and only reuses an entry when
  the times match, which is what the exe-path cache already did.
- `affinity_matches` read a `(0, 0)` affinity pair as "unconstrained" and could
  therefore skip a soft rule on the strength of a read that never happened:
  Windows never reports an affinity mask of 0, so a zero means the read failed.
  Zero masks now count as unknown and take the write path.

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
