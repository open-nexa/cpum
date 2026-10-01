# CPU Manager roadmap

This is the plan for CPU Manager, and — just as importantly — the list of things
that are deliberately *not* planned. It is written to be argued with: open an
issue and disagree with a priority rather than quietly doing something else.

### How to read this roadmap

- **Priorities** are `P0` / `P1` / `P2`. `P0` blocks the next release,
  `P1` is the release after it, `P2` is "yes, someone should, no, nobody has".
- **Phases** are named after what they buy the user, not after a version number.
  A phase ships when its items are done, not on a date.
- **Non-goals** at the end are the interesting part. If your idea is listed
  there, the section says why; that reasoning is what to attack if you disagree.

## 1. Where CPU Manager stands

CPU Manager is a working, released Windows desktop application. It does the thing
its name says: you can inspect CPU topology, see every running process with live
metrics, and change affinity, CPU sets and three kinds of priority, either once
or through saved rules that an optional `LocalSystem` service keeps applied.

What is genuinely solid:

- The Win32 layer is written once, in `cpum-core::procwin`, and shared by the GUI,
  the service and the one-shot elevated helper. There is one implementation of
  "read or write an affinity mask", not three.
- The privilege model is deliberate and mostly right: the GUI never elevates, the
  service does the privileged work over a named pipe guarded by a per-user token,
  and a UAC prompt is a fallback rather than the normal path.
- Multi-processor-group machines (>64 logical processors) are handled with a mask
  per group instead of pretending a single `u64` is enough. This is the part that
  most affinity tools get wrong.
- Rules survive upgrades: the store migrates v1 → v2 on read, and every persisted
  or event-sent field carries `#[serde(default)]` so an older build's file still
  parses.

What is not solid yet, and why:

- **ProBalance is experimental.** It is marked *Unstable* in the UI on purpose.
  The heuristic works, but the threshold defaults are guesses and the journal has
  no analysis tooling around it.
- **The installers are unsigned.** Code signing is pending a
  [SignPath Foundation](https://signpath.org/) certificate, so users hit a
  SmartScreen warning. This costs more downloads than any missing feature does.
- **ARM64 is built but not validated.** The release matrix produces an ARM64
  installer; topology detection (CCD grouping, hybrid P/E-core classification)
  has only ever been exercised on x64.
- **The rule engine polls.** The service re-applies rules every five seconds
  rather than subscribing to process-creation events, so a short-lived process
  can start, run and exit between two ticks without ever being pinned.
- **The backend speaks English.** Success and error strings returned from Rust
  bypass `src/i18n.ts`, so an English sentence can appear in a Chinese UI.

## 2. Self-review: what is missing

Ordered by what actually hurts.

### 2.1 Distribution and first impression (P0)

Nobody can use a tool they cannot install, and nobody trusts a README with no
picture in it.

- Signed installers, so SmartScreen stops intercepting the first run.
- Screenshots in both READMEs.
- A public download page per release, with the checksums on it.
- An `Install` section in the READMEs — until recently there was none, which is
  an odd thing to discover about a desktop application.

### 2.2 Correctness on machines that are not mine (P1)

- An ARM64 pass over `topology.rs`: confirm CCD detection and hybrid-core
  classification, and confirm that >64-LP ARM machines report groups correctly.
- Event-driven rule application (WMI `Win32_ProcessStartTrace`, or an ETW process
  provider) alongside the five-second tick, so short-lived processes are caught.
- Migrate the rest of a legacy data directory. `legacy_rules_dirs()` currently
  moves `affinity_rules.json` but leaves `probalance.json` and the journal behind.
- Localise backend strings behind error codes instead of raw English sentences.

### 2.3 Making ProBalance trustworthy (P1)

- Defaults derived from data rather than intuition: record real contention
  windows and pick thresholds that produce few false positives.
- A journal viewer that filters and summarises, instead of a raw JSONL tail.
- A dry-run mode that reports what *would* be downgraded, so the heuristic can be
  evaluated without letting it touch anything.

### 2.4 Test and review surface (P1/P2)

- All Rust tests live in `cpum-core`; the desktop and service binaries set
  `test = false`. The parts that most need tests — the matcher, the store
  migration, the ProBalance state machine — are covered, but the GUI crate's
  command layer is not.
- The frontend has no test runner. The highest-value test is not a component
  test: it is a guard that the two locale dictionaries stay in sync, because that
  is a house rule everyone forgets.
- No AI review configured yet.

## 3. Roadmap

### Sequencing principles

1. **A thing that stops people installing beats a feature.** Signing and
   screenshots come before new capabilities.
2. **Correctness on hardware I do not own comes before new heuristics.**
3. **Nothing becomes non-experimental until it has data behind it.** ProBalance
   loses the *Unstable* chip on evidence, not on optimism.

### Phase 0 — "reachable" (v0.2)

- [x] `Install` section and navigation in both READMEs.
- [x] Public download page with per-release checksums.
- [x] This roadmap.
- [x] An automated guard that `zh-CN` and `en-US` in `src/i18n.ts` stay in sync.
- [x] Screenshots in both READMEs (process list in flat and tree view, the rule
      manager, the ProBalance panel).
- [ ] Signed installers through SignPath.
- [ ] A short end-to-end demo: install the service, watch a rule get reapplied
      after a restart.

### Phase 1 — "correct on more machines" (v0.3)

- [ ] ARM64 topology validation, with a fixture so a regression fails in CI.
- [ ] Event-driven rule application in the service.
- [ ] Full legacy-directory migration (rules, ProBalance config, journal).
- [ ] Localised backend messages.
- [ ] `Cargo.toml` MSRV and a pinned toolchain, so "works on my machine" has a
      version number attached to it.

### Phase 2 — "explainable" (v0.4)

- [ ] ProBalance dry-run mode and a journal summariser.
- [ ] Threshold defaults backed by recorded data.
- [ ] Rule import / export, and a `--dry-run` in the one-shot service helper that
      prints what a rule set would do.
- [ ] Per-CCD and hybrid-core affinity presets in the editor, so "pin to the
      performance cores" is one click rather than a mask calculation.

### Phase 3 — "stable" (v1.0)

- [ ] ProBalance out of experimental, or removed if the data does not support it.
- [ ] A stable rule schema with a documented deprecation policy.
- [ ] Documented, versioned IPC contract between GUI and service.

## 4. Non-goals

Each of these has been considered. The reason is the part worth reading.

- **A Linux or macOS port.** Affinity, CPU sets, I/O priority and memory priority
  are Win32 concepts; `cpum-core::procwin` is a thin, careful wrapper over them.
  A port would mean writing a different program with the same name. If someone
  wants a Linux equivalent, `taskset`, `cgroups` and `sched_setaffinity` already
  cover most of it, and a small wrapper around those is a better project.
- **A kernel driver.** Everything here is doable from user mode. A driver would
  raise the privilege floor, break the "the GUI never elevates" property, and
  require signing that an individual maintainer cannot get.
- **Overriding protected processes.** PPL processes (`csrss`, some anti-cheat)
  cannot be modified at *any* privilege level. That is the OS doing its job. The
  app reports the failure as-is rather than pretending it can work around it.
- **Managing anything other than the CPU.** No GPU, disk scheduler, or NUMA
  memory policy management. Each is a separate domain with its own failure modes,
  and a tool that half-does four things is worse than one that does one.
- **Telemetry.** No crash reporting, no usage counters, no phone-home. The
  updater checks a release manifest over HTTPS and that is the entire network
  surface of the application.
- **Accounts, sync, or a cloud rule library.** Rules are a JSON file in the user's
  own `%APPDATA%`. Adding an account to store affinity rules would be a strictly
  worse deal for the user.
- **A system-tray-only mode or a CLI-first rewrite.** The value is in seeing
  topology and processes together; that needs a window.

## 5. How this changes

Open an issue. If it is an item above, say you are taking it and link this
document. If it is a new idea, say which heading it belongs under and what it
displaces — a roadmap where everything is added and nothing is dropped is a
wish list.

Items in *Non-goals* are not rejected by default, but the bar is different: show
that the stated reason no longer holds.
