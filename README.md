# CPU Manager

[中文版](README.zh-CN.md)

[![CI](https://github.com/open-nexa/cpum/actions/workflows/ci.yml/badge.svg)](https://github.com/open-nexa/cpum/actions/workflows/ci.yml)
[![CodeQL](https://github.com/open-nexa/cpum/actions/workflows/codeql.yml/badge.svg)](https://github.com/open-nexa/cpum/actions/workflows/codeql.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

CPU Manager is a Windows desktop application for inspecting CPU topology, browsing running processes, and managing their CPU affinity, scheduling hints, and priorities. It combines a Vue 3 user interface with a Rust/Tauri backend and an optional Windows service that reapplies saved rules automatically. A foreground-aware **ProBalance** engine is included to keep the system responsive when the foreground process comes under CPU contention.

> This project uses Windows APIs and is intended for Windows only.

[Install](#install) · [Screenshots](#screenshots) · [Features](#features) · [Rules and ProBalance](#rules-probalance-and-the-service) ·
[Development](#development) · [Contributing](CONTRIBUTING.md) · [Roadmap](docs/ROADMAP.md) ·
[Changelog](CHANGELOG.md) · [Release notes](https://open-nexa.github.io/cpum/)

---

## Features

- Inspect the CPU topology: logical processors, physical cores, SMT threads, packages, and detected CCD/die information. Intel hybrid P/E-core distinction is preserved.
- Browse running processes in a hierarchical view with live CPU, memory, disk, and (on Win11 24H2+) network metrics.
- Change the affinity mask of an individual process using a topology-aware editor. Multi-processor-group systems are handled with per-group masks rather than a single 64-bit value.
- Create, enable, edit, delete, and apply persistent affinity rules.
- Rule matching modes:
  - **Exact** — case-insensitive process name (`.exe` is optional), the v1 behavior.
  - **Wildcard** — glob pattern against the process name (`code*`, `*steam*`, `?` as a single character).
  - **Path** — glob pattern against the full executable path (e.g. `C:\Games\*\game.exe`).
- Rule scheduling mode: **Strict** (hard `SetProcessAffinityMask`) or **Soft** (`SetProcessDefaultCpuSets`, Win10 1803+; the scheduler may temporarily drift the process to other cores under load).
- Rules may also pin **CPU priority class**, **I/O priority**, and **memory priority**. ProBalance automatically excludes processes whose priorities are managed by a rule so the two engines do not fight.
- ProBalance dynamic optimization: when the foreground process's CPU stays above a configurable threshold, background processes that exceed a CPU threshold are temporarily downgraded (priority class + I/O priority) and restored automatically once contention clears, the process exits, or the feature is disabled. The status, statistics, and JSONL journal are exposed in the GUI.
- Install an optional `CpumAffinityService` Windows service that starts automatically and scans for matching processes every five seconds. The same service hosts the ProBalance runtime, and doubles as a **privileged bridge**: the un-elevated GUI delegates changes to protected processes to it over a named pipe, so no UAC prompt is needed.
- Persist rules and ProBalance configuration in the per-user data directory `%APPDATA%\com.open-nexa.cpum\` (`affinity_rules.json`, `probalance.json`, journal/status files). Legacy locations (`%APPDATA%\com.eason.cpum`, `%ProgramData%\cpum`, `%APPDATA%\cpum`) are migrated on first run or install.

## Screens and workflow

1. Start CPU Manager and let it load the CPU topology and process list.
2. Open the **Process** tab to set a single process's affinity and priorities immediately, or switch to the **Rules** tab to save a reusable rule.
3. Use **Apply Rules** to apply every enabled rule to currently running matching processes.
4. Use the **ProBalance** tab to configure foreground contention thresholds, allowlists, and view the live status / log.
5. Install the Windows service from the application if rules and ProBalance should also be enforced after sign-in or reboot.

Changing affinity can affect responsiveness and throughput. Test masks carefully, especially on hybrid CPUs or machines running latency-sensitive workloads. ProBalance downgrades priorities only; it never kills or hard-pins background processes.

## Screenshots

<table>
  <tr>
    <td align="center"><img src="screenshots/processes-flat.png" width="420" alt="Process list in flat view"><br><sub>Process list — flat view, with the per-CCD affinity bars</sub></td>
    <td align="center"><img src="screenshots/processes-tree.png" width="420" alt="Process list in tree view"><br><sub>Process list — tree view</sub></td>
  </tr>
  <tr>
    <td align="center"><img src="screenshots/rules.png" width="420" alt="Rule manager"><br><sub>Rule manager</sub></td>
    <td align="center"><img src="screenshots/probalance.png" width="420" alt="ProBalance panel"><br><sub>ProBalance panel</sub></td>
  </tr>
</table>

More images (and the specification for capturing them) are in [screenshots/](screenshots/README.md).

## Install

Download the installer for your CPU architecture from the
[latest release](https://github.com/open-nexa/cpum/releases/latest), or pick an
older one from the [download page](https://open-nexa.github.io/cpum/):

| Architecture | Installer |
| --- | --- |
| x64 | `CPU-Manager_<version>_windows-x64-setup.exe` |
| ARM64 | `CPU-Manager_<version>_windows-arm64-setup.exe` |

To **run** CPU Manager you need Windows 10 or later and the WebView2 Runtime,
which ships with current Windows installations. The installer is per-user: it
installs into `%LOCALAPPDATA%` and never asks for administrator rights.

### SmartScreen and checksums

Installers are not Authenticode-signed yet — the project is going through the
[SignPath Foundation](https://signpath.org/) programme for a free certificate
(the application text lives in
[docs/signpath-foundation-application.md](docs/signpath-foundation-application.md)).
Until a signing certificate is attached to the release pipeline, Windows may show
**"Windows protected your PC"** the first time you run the installer. Choose
**More info → Run anyway**.

Every release publishes a `SHA256SUMS.txt`, so you can check what you downloaded
before running it:

```powershell
Get-Content .\SHA256SUMS.txt
(Get-FileHash .\CPU-Manager_v0.1.0_windows-x64-setup.exe -Algorithm SHA256).Hash.ToLower()
```

The two hashes must match. If they do not, delete the file — do not run it.

### Upgrading

Installations of **0.1.1 and older** check the project's previous repository for
updates and will never see a new release. Install **0.2.0** once by hand; from
there the built-in updater takes over and offers every later version
automatically, verifying a minisign signature before it installs anything.

Uninstalling from *Apps & Features* also removes `CpumAffinityService` if it was
installed. Your rules stay in `%APPDATA%\com.open-nexa.cpum\`.

## Requirements (development)

- Windows 10 or later
- Node.js and npm
- npm (the Tauri dev hook runs `npm run dev`)
- Rust stable with the MSVC toolchain
- Microsoft C++ Build Tools / Visual Studio Build Tools (required by the Rust Windows toolchain)
- WebView2 Runtime (normally included with current Windows installations)

The desktop app runs with the privileges of the invoking user (the binary embeds an `asInvoker` manifest) and the installer is a per-user NSIS installer that installs into `%LOCALAPPDATA%` without a UAC prompt. Only Windows service management (install / uninstall / start / stop) needs administrator rights: those commands are re-launched through an elevated helper, so a single UAC prompt appears for that action only.

Because the app is un-elevated, some processes are protected by Windows or belong to another security context and may reject affinity or priority changes; install the service (which runs as `LocalSystem`) to cover those cases.

## Development

Install the JavaScript dependencies:

```powershell
npm ci
```

Start the app in development mode:

```powershell
npx tauri dev
```

Run the frontend type check and Rust compilation check independently:

```powershell
npx vue-tsc --noEmit
cd src-tauri
cargo check
```

## Production build

The release script refreshes the application icons, builds the frontend, compiles `cpum_service.exe` from the workspace, and produces a per-user NSIS installer:

```powershell
.\build.bat
```

The installer is written to:

```text
src-tauri\target\release\bundle\nsis\CPU Manager_0.1.0_x64-setup.exe
```

Alternatively, build the frontend and Tauri bundle manually. Build the Windows service binary first so it can be picked up as a Tauri bundle resource (the path is declared in `tauri.conf.json`):

```powershell
npm run build
cd src-tauri
cargo build --release -p cpum-service --bin cpum_service
cd ..
npx tauri build --bundles nsis
```

## Rules, ProBalance, and the service

### Rule schema (v2)

Rule files use a v2 envelope and live at:

```text
%APPDATA%\com.open-nexa.cpum\affinity_rules.json
```

The on-disk format:

```json
{
  "version": 2,
  "rules": [
    {
      "id": "<uuid>",
      "process_name": "code",
      "mask": "0xFF",
      "group_masks": ["0xF", "0xF0"],
      "enabled": true,
      "created_at": 1736000000,
      "note": "front-end on P-cores",
      "match_type": "exact",
      "mode": "strict",
      "priority_class": "0x80",
      "io_priority": 1,
      "memory_priority": 5
    }
  ]
}
```

- `group_masks` is optional. When omitted, `mask` is treated as the legacy group-0 value, so every existing rule file is still readable.
- `match_type` is one of `exact` (default), `wildcard`, or `path`.
- `mode` is one of `strict` (default) or `soft`.
- `priority_class`, `io_priority`, and `memory_priority` are all optional; `null`/missing means "do not adjust". When any of them is set, ProBalance treats the matched process as protected and will not downgrade it.
- The legacy v1 format (a bare JSON array) is still parsed and auto-migrated to v2 defaults on read; the file is rewritten in v2 form on the next save.

### Windows service

The application can install, start, stop, and uninstall the `CpumAffinityService` service. The service runs as `LocalSystem`, starts automatically, receives the rule directory (`%APPDATA%\com.open-nexa.cpum`) when installed, and hosts both the rule engine and the ProBalance runtime. It attempts to enable `SeDebugPrivilege` so it can apply rules to processes in user sessions.

Elevation is scoped to this action: the app calls `sc.exe` through an elevated helper, so installing or removing the service shows one UAC prompt while the rest of the app keeps running un-elevated.

To verify an installation from a PowerShell prompt:

```powershell
sc.exe qc CpumAffinityService
sc.exe query CpumAffinityService
Get-Content "$env:APPDATA\com.open-nexa.cpum\affinity_rules.json"
```

`Running` only confirms that the service is running; it does not prove that a rule matched a process or that Windows accepted its affinity mask. Confirm the service binary path and arguments, the rule file, and the target process affinity when troubleshooting.

### Privileged operations (protected processes)

The desktop app runs un-elevated, so `OpenProcess` is refused for processes owned by another account or running at a higher integrity level (a filtered UAC token does not hold `SeDebugPrivilege`). When that happens the app escalates in two steps:

1. **Service bridge** (preferred) — the request is forwarded to `CpumAffinityService` over the named pipe `\\.\pipe\cpum-bridge-v1`. The service runs as `LocalSystem` with `SeDebugPrivilege`, so the change is applied with no prompt at all. **Apply Rules** also uses this path for the processes the GUI could not handle.
2. **One-off elevation** (fallback) — when the service is not installed, the bundled `cpum_service.exe` is re-launched elevated with `--set-affinity` / `--set-priority`, which raises a single UAC prompt for that one change. A PID that fails even then (PPL / protected anti-cheat) is remembered for the session so it does not prompt again.

Authorization: the pipe DACL allows SYSTEM and interactive users, but every request must carry a random token stored in `%APPDATA%\com.open-nexa.cpum\bridge.token` — a file only that user (and SYSTEM) can read. A different local user can open the pipe but cannot produce a valid token.

Note that **PPL processes cannot be modified at all** (not even by an elevated administrator); those will always fail, and the error is surfaced as-is.

For a one-time diagnostic application of the rules, run the installed service executable with:

```powershell
& "<path-to-cpum_service.exe>" --apply-once "$env:APPDATA\com.open-nexa.cpum"
```

## Auto-update and release signing

Releases are published as signed updater artifacts. The Tauri updater plugin
is wired up in the desktop binary; the GitHub Actions release pipeline emits
the signed `.nsis.zip` archives, the matching minisign signatures, a
multi-architecture `latest.json` manifest, and a `SHA256SUMS.txt` file with
the SHA-256 of every released file. Key generation, CI secrets, and
verification instructions live in [docs/UPDATER.md](docs/UPDATER.md).

## Project layout

```text
src/                              Vue 3 frontend
  components/                     Affinity editor, rule manager, ProBalance panel
  composables/                    Topology, process, metrics, process-tree state
  api.ts                          Tauri IPC wrappers
  i18n.ts                         Bilingual (zh-CN / en-US) dictionary
src-tauri/                        Tauri desktop binary
  src/
    lib.rs                        Tauri commands (topology, process, rules, ProBalance)
    models.rs                     Shared serde data models
    process/                      Process enumeration, metrics, sampling
    topology.rs                   CPU topology detection
  crates/
    cpum-core/                    Rule model, matcher, store, ProBalance, procwin, IPC bridge
    cpum-service/                 cpum_service.exe (Windows service + ProBalance runtime
                                  + privileged bridge server + one-shot elevated helper)
  installer-hooks.nsh             NSIS hooks: migrate legacy rule files, elevate only for service ops
  tauri.pubkey                    Committed updater public key (private key is gitignored)
docs/
  ROADMAP.md                      What is solid, what comes next, and the non-goals
  UPDATER.md                      Key management, release artifacts, verification
  signpath-foundation-application.md  Text used to apply for free code signing
  releases/                       The download page (published to GitHub Pages)
scripts/
  lint-i18n.mjs                   Guards the zh-CN / en-US dictionaries
  snapshot-releases.mjs           Refreshes the download page's fallback data
screenshots/                      README images; see the spec in that directory
.github/                          CI / CodeQL / Pages workflows, issue + PR templates, dependabot
CONTRIBUTING.md                   Build setup, house rules, good first issues
SECURITY.md                       Privilege boundary and vulnerability reporting
CHANGELOG.md                      Notable changes, Keep a Changelog format
build.bat                         NSIS release build script
```

## Notes and limitations

- On multi-processor-group systems, rules carry a `group_masks` array (one entry per active group). The single-value `mask` field remains the legacy group-0 view and is kept for backward compatibility.
- Network I/O counters require Win11 24H2+ (NtQueryInformationProcess / `ProcessNetworkIoCounters`); on older Windows versions the values are reported as zero.
- Saving a rule does not automatically apply it until you use **Apply Rules** or the service detects a matching process.
- ProBalance only downgrades CPU/IO priority; it never modifies affinity masks. Rule-managed priorities are excluded from ProBalance actions to keep the two engines from undoing each other.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) has the build setup, the house rules and a
list of good first issues. Read it before opening a pull request — the two that
catch people out are that every user-visible string has to be added to **both**
locales in `src/i18n.ts`, and that comments are English only.

## Security

The desktop app runs un-elevated; only service management elevates, and the GUI
reaches processes it cannot open through the service over a named pipe instead
of raising a UAC prompt. [SECURITY.md](SECURITY.md) describes that boundary,
what is known to be out of its reach (PPL processes cannot be modified at any
privilege level), and how to report a vulnerability privately.

## Changelog

Notable changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## License

CPU Manager is licensed under the [MIT License](LICENSE).
