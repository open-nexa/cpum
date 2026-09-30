# Repository Guidelines

## Project Structure & Module Organization

This is a **Tauri desktop application** for managing CPU affinity on Windows.

```
cpum/
├── src/                                # Vue 3 + TypeScript + Vuetify frontend
│   ├── api.ts                          # Tauri IPC command wrappers
│   ├── App.vue                         # Main application shell (orchestrator)
│   ├── i18n.ts                         # Bilingual (zh-CN / en-US) message dictionary & t() helper
│   ├── types.ts                        # Shared TypeScript types (mirrors Rust models)
│   ├── constants.ts                    # Shared constants and type aliases
│   ├── vuetify.ts                      # Vuetify plugin setup (theme + icons)
│   ├── components/
│   │   ├── AffinityEditor.vue          # Per-process affinity editor dialog
│   │   ├── AffinityRuleManager.vue     # Persistent rule manager dialog
│   │   └── ProBalancePanel.vue         # ProBalance dynamic-optimization settings panel
│   └── composables/
│       ├── useTopology.ts              # CPU topology cache + refresh lifecycle
│       ├── useProcessManager.ts        # Process list bootstrap, refresh, search
│       ├── useMetricsStream.ts         # Real-time metrics event handling
│       ├── useProcessTree.ts           # Process tree/flattening logic
│       └── useTheme.ts                 # Dark/light theme switcher (localStorage: cpum-theme)
├── src-tauri/                          # Rust backend (cargo workspace root)
│   ├── src/
│   │   ├── lib.rs                      # Tauri command registrations & entry point
│   │   ├── models.rs                   # Shared data models (serde)
│   │   ├── topology.rs                 # CPU topology detection (CCD, cores, SMT)
│   │   └── process/                    # Process subsystem (GUI-side)
│   │       ├── sampling.rs             # Sampling primitives + differential cache
│   │       ├── enumerate.rs            # ToolHelp fast scan + parallel handle walk
│   │       ├── metrics.rs              # Per-second metrics stream (4-wave events)
│   │       ├── net_probe.rs            # Network IO diagnostic probe
│   │       └── system_metrics.rs       # Logical-processor usage sampling
│   └── crates/
│       ├── cpum-core/                  # Shared core library (no tauri dependency)
│       │   └── src/
│       │       ├── rule.rs             # Rule data model (schema v2) & validation
│       │       ├── matcher.rs           # Name/path matching (exact / wildcard / path)
│       │       ├── store.rs             # Rule file persistence (v1→v2 auto-migration)
│       │       ├── procwin.rs           # Win32 writes: affinity / CPU Sets / 3 priority classes
│       │       ├── engine.rs            # Rule application engine (enumerate→match→apply)
│       │       ├── monitor.rs           # CPU differential sampling + foreground detection
│       │       ├── ipc.rs               # Named-pipe privileged bridge (GUI↔service)
│       │       └── probalance/          # Dynamic optimization engine (config, engine,
│       │                                #   journal, runtime, tests — split by responsibility)
│       └── cpum-service/                # Windows service binary (LocalSystem daemon)
│           └── src/main.rs              # Rule daemon + ProBalance tick + one-shot helper
├── .github/                            # CI / CodeQL workflows, issue + PR templates, dependabot
├── .gitleaks.toml                      # Secret-scan allowlist (public keys that look like keys)
├── CHANGELOG.md                        # Keep a Changelog; add to [Unreleased] with every PR
├── CONTRIBUTING.md                     # Build setup, house rules, good first issues
├── SECURITY.md                         # Privilege boundary and vulnerability reporting
├── CODE_OF_CONDUCT.md
└── package.json
```

## Build, Test, and Development Commands

| Command | Description |
|---------|-------------|
| `npm ci` | Install frontend dependencies. **npm is the only supported package manager** — there is no `yarn.lock` |
| `cd src-tauri && cargo build --release -p cpum-service --bin cpum_service` | Produce `target/release/cpum_service.exe`. **Required before any `cargo check` / `cargo clippy` of the desktop crate**: `tauri-build` treats it as a bundle resource and fails when it is missing |
| `cd src-tauri && cargo check` | Check all Rust code (workspace) for compilation errors |
| `cd src-tauri && cargo test -p cpum-core` | Run the Rust unit tests (all tests live in cpum-core) |
| `cd src-tauri && cargo clippy --workspace --all-targets -- -D warnings` | Lint gate; CI keeps warnings at zero |
| `cd src-tauri && cargo fmt --all -- --check` | Report rustfmt drift only |
| `npx vue-tsc --noEmit` | Type-check frontend TypeScript/Vue files |
| `npm run build` | `vue-tsc --noEmit` + `vite build` |
| `npx tauri dev` | Run the full Tauri app in development mode |
| `.\build.bat` | Full local release build: icons, frontend, service binary, NSIS installer, `SHA256SUMS.txt` |

`.github/workflows/ci.yml` runs the Rust, frontend, bundle, audit and secret-scan
jobs on every pull request. `.github/workflows/release.yml` publishes the
installers when a `v*` tag is pushed.

## Coding Style & Naming Conventions

- **Rust**: Standard `rustfmt` style. `snake_case` for functions/variables, `PascalCase` for types.
- **TypeScript/Vue**: Vue 3 `<script setup>` syntax. Components use `PascalCase` filenames. Composables prefixed with `use`.
- **Indentation**: 2 spaces (frontend), 4 spaces (Rust).
- **Encoding**: All files must be **UTF-8 without BOM**. Never use PowerShell `Set-Content` for non-ASCII content — use Python or Node.js instead.
- **i18n (MANDATORY)**: ALL user-visible strings MUST go through the dictionary in `src/i18n.ts` — never hardcode Chinese or English UI text in `.vue` templates, `<script>` logic, or composables. Details below.

## Internationalization (i18n)

The UI is bilingual (**zh-CN / en-US**), managed by `src/i18n.ts`:

- **Dictionary**: every message is a key with entries in both `messages["zh-CN"]` and `messages["en-US"]`. When adding a key, add it to **both locales** (keep them in sync).
- **Interpolation**: `t()` supports `{param}` placeholders — `t("rulesAppliedTo", { count })`. Use this instead of string concatenation.
- **Usage**:
  - In components: `const { t, toggleLocale } = useI18n()` — `t` is reactive, so texts update instantly when the locale toggles.
  - Outside components (composables / plain `.ts`): `import { t } from "../i18n"` — the module-level `t` reads the current locale ref directly.
- **Persistence**: locale is stored in `localStorage` under key `cpum-locale`.
- **Code comments must be in English**: this is an open-source project, so all non-user-facing comments (Rust `//` / `///` / `//!`, TypeScript `//` / `/* */`, Vue templates, Markdown, and user-visible Rust strings such as `eprintln!` / `format!` / `panic!` macros) MUST be in English. Only the zh-CN translation values inside `src/i18n.ts` and the Chinese-localized `README.zh-CN.md` stay in Chinese by design.
- **Technical terms stay as-is in both locales**: `PID`, `LP`, `CCD`, `Mask`, `0xFF` etc.
- **Known limitation**: success messages returned by the Rust backend (e.g. service install/start) bypass the frontend dictionary; translating them requires backend-side changes.
- **Verification**: after i18n changes, run `npx vue-tsc --noEmit` and grep the repo tree to confirm no Chinese string literals remain outside `i18n.ts` (zh-CN values) and `README.zh-CN.md` (the intentionally Chinese readme).

## Architecture Notes

- **Composables**: Each composable manages a single concern (`useTopology`, `useProcessManager`, `useMetricsStream`, `useProcessTree`, `useTheme`). App.vue orchestrates them.
- **Data flow**: Frontend calls Tauri commands via `invoke()` in `api.ts`. Rust backend executes Windows API calls and returns results.
- **Events**: Backend pushes real-time metrics via `app.emit()`; frontend subscribes with `listen()`.
- **Workspace layout**: `src-tauri` is a cargo workspace with two member crates. `cpum-core` holds the rule model / matching / persistence / Win32 write operations / rule engine / ProBalance — it has **no tauri dependency** so the service binary can reuse it. `cpum-service` is the Windows service binary; keeping it a separate crate avoids the Tauri build script's chicken-and-egg on the bundled `cpum_service.exe` resource. Affinity / CPU Sets / priority reads and writes are implemented **once** in `cpum-core::procwin` — never reimplement them in the GUI crate.
- **Process subsystem split**: `src-tauri/src/process/` is split by responsibility (`sampling` → `enumerate` → `metrics`, plus `net_probe` and `system_metrics`); keep external APIs unchanged via `pub use` re-exports in `mod.rs`.
- **Service daemon**: runs as LocalSystem — every 5 s it scans processes and applies rules; every 1 s it runs a ProBalance decision tick (foreground contention detection → background downgrade → auto-restore). Rules that manage priorities form a "protected list"; matching processes are skipped by ProBalance so the two engines never clobber each other.
- **Process rules**: Persisted as JSON at `%APPDATA%/com.open-nexa.cpum/affinity_rules.json` (the Tauri per-user data directory, derived from the bundle identifier in `tauri.conf.json`; the same directory holds `probalance.json` and the ProBalance journal/status files). Read-only migration sources for older releases — `%APPDATA%\com.eason.cpum` (the previous bundle identifier), `%APPDATA%\cpum` and `%PROGRAMDATA%\cpum` — are listed in `legacy_rules_dirs()`; add a new one there rather than changing any write path. A rule can manage affinity (hex mask + strict/soft schedule mode) and CPU/IO/memory priorities; processes are matched by exact name, wildcard, or full executable path (case-insensitive).
- **Privilege model**: the desktop app runs un-elevated (`asInvoker` manifest) and ships as a per-user NSIS installer into `%LOCALAPPDATA%` (`installMode: "currentUser"`). Only Windows service management needs administrator rights: `install_service` always re-launches `sc.exe` through an elevated helper, and `uninstall/start/stop_service` fall back to elevation on access-denied. The NSIS hooks in `installer-hooks.nsh` touch the service only when it already exists, so a fresh install (and a plain app update without the service) shows no UAC prompt.
- **Never move rules/config back to a machine-level directory** such as `%ProgramData%`: an un-elevated GUI cannot write there, and the service reads the directory passed as its startup argument.
- **Service data directory**: `install_service` writes the per-user directory into the service `ImagePath` (`sc create/config binPath= "<exe>" "<dir>"`). The SCM hands an `ImagePath` argument to the **process command line only** — `ServiceMain`'s `lpServiceArgVectors` carries just the service name plus whatever a `StartService` caller passed. Always resolve the directory through `cpum_core::service_dir::resolve` (it checks both channels, then the legacy `%ProgramData%\cpum`); reading `ServiceMain`'s vector alone makes the service run against a stale directory, which the GUI shows as "service offline" and which silently disables the token-authenticated IPC bridge.
- **Privileged fallback**: `set_process_affinity` / `set_process_priority` first run locally; on `ERROR_ACCESS_DENIED` they go through `cpum_core::ipc` (named-pipe bridge to the LocalSystem service, token-authenticated) and only then fall back to a one-off elevated launch of `cpum_service.exe --set-affinity/--set-priority`. `apply_affinity_rules` uses the bridge for the processes it could not handle, but never the UAC path (a batch operation must not raise one prompt per process).
