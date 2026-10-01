# Contributing to CPU Manager

Thanks for looking. This document is the short version of how to get from a
clone to a merged change, plus a list of work that is genuinely open.

## Where things live

| Directory | What it is |
| --- | --- |
| `src/` | Vue 3 + TypeScript frontend (Vuetify) |
| `src-tauri/src/` | The desktop binary: Tauri commands, models, process sampling, topology |
| `src-tauri/crates/cpum-core/` | Rule model, matcher, store, Win32 writes, ProBalance, IPC bridge. No `tauri` dependency, so the service can reuse it |
| `src-tauri/crates/cpum-service/` | `cpum_service.exe` — Windows service, ProBalance runtime, privileged-bridge server, one-shot elevated helper |

Both Rust trees are one cargo workspace rooted at `src-tauri/`.

```bash
git clone https://github.com/open-nexa/cpum.git
```

## Getting set up

CPU Manager is Windows-only. You need:

- Windows 10 or later
- Node.js 22 and npm
- Rust stable with the MSVC toolchain, plus Microsoft C++ Build Tools
- WebView2 Runtime (already present on current Windows)

```bash
npm ci
npx tauri dev
```

`npx tauri dev` runs `npm run dev` for the frontend and then the desktop binary.

## Build order matters

`tauri.conf.json` declares `target/release/cpum_service.exe` as a bundle
resource, and the build script fails if it is missing. Build the service binary
**before** the desktop bundle:

```bash
npm run build
cd src-tauri
cargo build --release -p cpum-service --bin cpum_service
cd ..
npx tauri build --bundles nsis
```

`build.bat` does all of this, including icons and a `SHA256SUMS.txt`. Without
`src-tauri/tauri.key` it skips updater signing; that is expected locally.

## Before you open a pull request

```bash
cd src-tauri
cargo fmt --all -- --check          # report drift only
cargo clippy --workspace --all-targets
cargo test -p cpum-core             # all Rust tests live in cpum-core
cd ..
npm run lint:i18n                   # zh-CN / en-US parity, t() keys, no stray CJK
npx vue-tsc --noEmit
```

CI runs the same set on every pull request, on `windows-latest`.

## House rules

- **i18n is mandatory.** Every user-visible string goes through the dictionary
  in `src/i18n.ts`, with entries in both `zh-CN` and `en-US`. No hardcoded UI
  text in `.vue` templates, `<script>` blocks or composables. Technical terms
  (`PID`, `LP`, `CCD`, `Mask`, `0xFF`) stay as-is in both locales.
- **Comments are in English**, including `eprintln!` / `format!` strings on the
  Rust side. Only the `zh-CN` values in `src/i18n.ts` and `README.zh-CN.md` are
  Chinese.
- **Files are UTF-8 without BOM.** Do not write non-ASCII files with PowerShell
  `Set-Content`.
- **New persisted or event fields need `#[serde(default)`.** Rules, the
  `ProcessInfo` cache and event payloads are read back from disk and from older
  builds; a missing `default` silently yields an empty list instead of an error.
  The same applies on the TypeScript side: a field the frontend reads must exist
  in the Rust payload struct.
- **Never send `null` for a Tauri command argument that is not `Option<T>`.**
  Tauri deserialises command arguments strictly.
- Affinity / CPU-set / priority reads and writes are implemented once, in
  `cpum-core::procwin`. Do not reimplement them in the GUI crate.
- New behaviour comes with a test. `cpum-core` is the only crate with Rust
  tests; the service entry point and the desktop binary both set `test = false`
  because neither is a unit-test harness.

## Good first issues

Bounded, real, and each one is useful on its own. Comment before you start and
say which one you are taking.

**1. Migrate more than `affinity_rules.json` from a legacy data directory.**
`legacy_rules_dirs()` (`src-tauri/src/lib.rs`) migrates the rule file when the
primary one is missing, but `probalance.json` and the ProBalance journal in the
same directory are left behind. Small, and it is the same shape as the code
that is already there.

**2. Screenshots, then a short demo.** Neither README has a single image, which
for a desktop application is worse than any missing feature. `screenshots/README.md`
lists the four shots needed and how to capture them. After that: no end-to-end
walkthrough exists either — one recording of installing the service and watching a
rule get reapplied after a restart would do more for the project than most of the
items here. No Rust required for either.

**3. Localise the backend success strings.** Messages returned by the Rust
backend (service install / start / stop) bypass `src/i18n.ts`, so they are
always English. Moving them behind a locale-aware error code fixes that.

**4. ARM64 correctness pass.** The release matrix builds an ARM64 installer, but
topology detection (CCD grouping, hybrid-core classification) has only been
exercised on x64. *Medium.*

**5. Make `latest.json` point at a file that exists.** The updater manifest for
v0.1.0 references `CPU Manager_0.1.0_x64-setup.exe` with a space, while GitHub
published the asset as `CPU.Manager_0.1.0_x64-setup.exe`. Confirm whether GitHub
resolves that URL; if it does not, the release workflow has to publish the
manifest with the name the asset actually has. *Small, but it needs a real
release to test against.*

The longer-term plan, including what is deliberately **not** planned and why, is
in [docs/ROADMAP.md](docs/ROADMAP.md).

## Labels

| Label | Means |
| --- | --- |
| `good first issue` | Bounded, self-contained, mentorship available |
| `help wanted` | Worth doing, but nobody on the project has time for it |
| `bug` / `enhancement` / `documentation` | Type of work |
| `security` | Touches the privilege boundary described in SECURITY.md |
| `needs-triage` | Nobody has looked at it yet |
| `area:core` `area:desktop` `area:service` `area:frontend` | Which component |

GitHub has no label file in the repository, so they are created through the API:

```bash
gh label create "good first issue" --color 7057ff --description "Bounded, self-contained, mentorship available"
gh label create "help wanted"      --color 008672 --description "Worth doing, no maintainer has time for it"
gh label create "security"         --color b60205 --description "Touches the privilege boundary"
gh label create "needs-triage"     --color ededed --description "Nobody has looked at it yet"
for a in core desktop service frontend; do
  gh label create "area:$a" --color c5def5 --description "Component: $a"
done
```

## Reporting security issues

See [SECURITY.md](SECURITY.md). If you find something exploitable, do not open a
public issue — report it privately.
