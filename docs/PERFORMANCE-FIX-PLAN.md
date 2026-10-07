# Performance audit — fix plan

Companion to `PERFORMANCE.md`, the measurement report behind this plan. That
document measures; this one says what to change, in what order, and how to prove
each change worked.

**`PERFORMANCE.md` is not committed, on purpose.** It is a working document and
lives outside the tree, in `.workbuddy/` next to this repository; its numbers are
re-measured per machine, so committing them would let the audit and the code drift
apart silently. Every mention of it below is therefore plain text and not a link.

Read it first. Its verdict is that CPU Manager is *not a measurable load* (0.31 %
of one core with the GUI closed, ~1.8 % of one core while streaming).
So nothing here is urgent, and the ordering below is by risk-adjusted value, not by
milliseconds — the one item that is worth doing for its own sake is **O4**, because
it changes what the tool *does to other processes*, not how fast the tool runs.

## 0. Inventory

| ID | Item | Where | Saving | Status |
|---|---|---|---|---|
| **O1** | Reuse the caller's handle for the network counters | `src-tauri/src/process/net_probe.rs`, `sampling.rs::read_net_counters_for_handle` | ~0.9 ms/round (~5 %) | done |
| **O5** | Drop the parallel walk (gate it on process count) | `src-tauri/src/process/enumerate.rs` | ~1 ms/round, 8 thread spawns/s | done |
| **O4** | Make rule application idempotent (read-compare-skip) | `src-tauri/crates/cpum-core/src/engine.rs::apply_one`, `procwin.rs` | removes repeated thread re-placement | done |
| **O2** | Cache the resolved exe path per PID | `enumerate.rs` (`EXE_PATH_CACHE`) | ~1.5 ms/round (~8 %) | done |
| **O3** | Read the three priority classes every 2 s, not every second | `enumerate.rs` (`PRIORITY_CACHE`) | ~1.9 ms/round (~10 %) | done |
| **O7** | Align the three front-end timers | `src/App.vue`, `src/components/ProBalancePanel.vue` | minor | done |
| **O6** | Cheaper process-table rows (fewer live components) | `src/components/ProcessTable.vue` (replaces `v-data-table` in `src/App.vue`) | frontend frame cost | **done** - virtual scrolling, step 3 |

**Numbers are still the audit's, not re-measured.** Every saving above is the
estimate from `PERFORMANCE.md`; see [§7](#7-implementation-status) for what has to
be re-run on the reference machine before those rows can be updated.

Baseline to beat: **18.4 ms** per metrics round at 388 processes; the audit
estimates **~14.1 ms** after O1–O3.

## 1. Ordering principles

1. **Semantics before microseconds.** O4 is the only item that changes behaviour
   another process can feel, so it outranks purely cosmetic savings.
2. **Provably-safe wins first.** O1 and O5 cannot change observable behaviour;
   they also move the baseline, so every later measurement is against them.
3. **Anything that can go stale needs an invalidation rule and a forced path.**
   O2 and O3 both cache; each ships with an explicit "when do we re-read" rule and
   at least one caller that bypasses the cache.
4. **Front end last, and measured.** JS is 0.31 ms/round — the visible cost is DOM,
   so no front-end change lands without a before/after DevTools recording.
5. **Every PR re-measures.** Update the numbers in `PERFORMANCE.md` in the same PR
   (the doc is a measurement, not a promise) and add a line to `CHANGELOG.md`
   under `[Unreleased]`.

## 2. Work items

### O1 — Reuse the handle the caller already owns (~0.9 ms/round)

**Now:** `sampling.rs::read_metrics_for_handle` already holds an open handle, then
`read_net_counters_for_handle` throws it away — it calls `GetProcessId(handle)` and
`net_probe::read_network_io(pid)`, which opens a *second* handle with
`PROCESS_QUERY_INFORMATION`.

**Change:**

- `net_probe.rs`: add `read_network_io_with_handle(handle: HANDLE) -> Option<(u64, u64)>`;
  rewrite `read_network_io(pid)` as *open → delegate → close* so the diagnostic
  probe (`dump_net_io_probe`) keeps working unchanged.
- `sampling.rs`: `read_net_counters_for_handle` calls the handle variant directly.
  Drop the `GetProcessId` round-trip.

**Why it is also a correctness fix:** the second open asks for
`PROCESS_QUERY_INFORMATION`, which fails more often than the caller's handle (the
caller already falls back to `PROCESS_QUERY_LIMITED_INFORMATION`). Reusing the
handle is both cheaper *and* succeeds more often, so the network column should
report non-zero for more processes than it does today.

**Acceptance:** net columns unchanged or better on Win11 24H2+; `dump_net_io_probe`
shows the same or fewer access-denied entries; round time drops ≈0.9 ms.

**Risk:** none. **Rollback:** revert to the pid-based open.

### O5 — Single-threaded walk by default

**Now:** `enumerate_with_snapshots` always splits the raw list across
`nproc.clamp(2, 8)` scoped threads. The audit measured 11.3 ms with 8 threads vs
10.1 ms single-threaded: the spawn plus synchronisation costs more than the 388 ×
26 µs it parallelises.

**Change:**

- Run the walk inline. Keep the parallel path behind
  `const PARALLEL_THRESHOLD: usize = 600;` — use threads only when
  `raw.len() >= PARALLEL_THRESHOLD` (above that the spawn cost amortises; below it
  does not).
- With the inline path the final `all_results.sort_by_key` is redundant (order is
  already the ToolHelp order) — drop it there, keep it on the parallel branch.

**Acceptance:** round time is not worse than the parallel build; process order in
the table is identical to the current build (the existing `system_process_first`
sort in `list_processes` is unchanged).

**Note:** re-measure *after* O1–O3. They remove per-process work, which makes
parallelism even less attractive; if the threshold turns out to be pointless,
delete the parallel branch entirely rather than keeping dead code behind a
constant nobody hits.

### O4 — Idempotent rule application (highest semantic value)

**Now:** `engine.rs::apply_one` writes affinity and the three priority classes
**unconditionally**, every 5 s from the service (and on every GUI "Apply Rules"
press). Every write makes the kernel re-evaluate thread placement for every thread
of that process, which can force migrations and cost cache locality — exactly what
a user pins a game renderer or a browser to a CCD to avoid.

**Change:**

- `procwin.rs`: add `affinity_matches(pid, masks, mode) -> bool`.
  - *Strict:* compare `get_process_group_masks(pid)` (single-group machines can use
    `get_process_affinity`) against the desired masks.
  - *Soft:* compute the expected CPU-set ids with `cpu_set_ids_for_group_masks(masks)`
    and compare with `get_process_default_cpu_set_locations(pid)` mapped to ids.
  - **Read failure ⇒ return `false`.** Never skip on uncertainty; a process we
    cannot read is a process we must keep trying to write.
- `engine.rs`: add `priorities_match(current: &ProcessPriorities, rule) -> bool`,
  comparing **only the fields the rule manages** (compare the CPU class by value,
  not by `priority_class_rank` — that function is for ordering, not equality).
- `apply_one`: read current state first; when affinity matches *and* all managed
  priorities match, return the `ProcessApplyInfo` without writing.
- `ApplyReport`: add `skipped: u32`. `changed` keeps only real writes.
- `lib.rs::apply_affinity_rules`: return `applied + skipped` for the toast count
  — "Rules applied to N processes" stays true, because those processes *are* in the
  desired state. Keep emitting `process://affinity-updated` /
  `process://priority-updated` for `changed` only.

**Tests (all Rust tests live in `cpum-core`):** extract the decisions into pure
functions and unit-test them:

- `group_masks_equal` — equal masks; differing masks; different lengths.
- `priorities_match` — rule managing only `io_priority` ignores class and memory;
  an unreadable (`None`) field ⇒ no skip; a value the OS reports that we do not
  recognise ⇒ no skip.
- `affinity_matches` is Win32-bound, so keep it thin and put the logic in the pure
  helpers above it.

**Acceptance:** with 7 rules matching 15 processes and nothing else touching the
machine, the 2nd and later ticks perform **zero** writes (check the returned
report's `skipped` count, or watch `SetProcessAffinityMask` in Process Monitor).
Change a mask by hand (Process Explorer) ⇒ the next tick writes again. A protected
process whose read fails ⇒ still attempted every tick.

**Escape hatch:** add `--force` to the service one-shot helper
(`cpum_service --apply-once --force`) that bypasses the comparison, so a suspected
skip bug can be ruled out without a rebuild.

**Risk:** a bug here means rules silently stop being enforced — the worst failure
mode in the list. Mitigations: read-failure ⇒ write, pure-function tests, the
`--force` hatch. **Rollback:** revert the PR; no flag is left behind.

### O2 — Cache the resolved exe path per PID (~1.5 ms/round)

**Now:** `query_image_path_from_handle` runs for every process every round, and the
path is effectively immutable for the lifetime of a process.

**Change:** a module-level cache in `enumerate.rs`:
`EXE_PATH_CACHE: Lazy<Mutex<HashMap<u32, (u64, String)>>>` — key is the PID, value
is `(creation time, path)`.

- `sampling.rs::read_metrics_for_handle` already reads the creation `FILETIME` and
  currently throws it away. Surface it as `create_time: Option<u64>` on
  `ProcessBasicInfo`.
- A cache entry is used **only when the PID and the creation time both match**,
  which makes it immune to PID reuse. On a miss, query and insert.
- Prune dead PIDs each round (retain against the current raw PID set).

**Acceptance:** the exe-path tooltip and Path-type rules behave exactly as before;
kill-and-restart loops never show another process's path. The creation-time guard
is pure logic — unit-test `cached_path_is_valid(pid, create_time, entry)` rather
than trying to force PID reuse on a live machine.

**Risk:** medium *if* the validity token is wrong — which is why the token is the
creation time and not the PID alone.

### O3 — Read the three priority classes every 2 s, not every second (~1.9 ms/round)

**Now:** `read_priorities_with_handle` (`GetPriorityClass` +
`NtQueryInformationProcess(33)` + `GetProcessInformation`) runs per process per
round.

**Change:** `PRIORITY_CACHE: Lazy<Mutex<HashMap<u32, ProcessPriorities>>>` plus
`const PRIORITY_REFRESH_MS: u128 = 2000;` in `enumerate.rs`.

Read is **forced** when:

- there is no cache entry for the PID (a new process must be correct immediately),
- the last refresh is older than `PRIORITY_REFRESH_MS`,
- the caller passes `force` — `list_processes` (the first full frame and every
  manual full refresh) must be exact.

Otherwise reuse the cached value. Prune against the alive set as in O2.

**Consequence to accept:** a priority change made by another tool, or by ProBalance,
shows up in the GUI within ≤2 s instead of ≤1 s. Write that into the code comment
and into `PERFORMANCE.md`; do not let it become an undocumented regression.

**Acceptance:** the priority column still follows changes made in the affinity
editor (which refreshes on its own) and by ProBalance, within 2 s; a full refresh
is exact.

### O6 — Cheaper process-table rows (front end)

**Finding:** 0.31 ms of JS per round — the data handling is not the problem. The
cost is that all 388 rows are live, and each row builds a `v-tooltip`, two
`v-icon`s, a `v-chip` (tree mode) and one `div` per CCD. That is thousands of
component instances patched every second.

**Decided: step 3, skipping steps 1 and 2.** Steps 1 and 2 were the incremental
options; step 1 removes ~388 tooltip instances but leaves every row live, and step 2
(pagination) changes the UX by hiding processes behind pages. Virtual scrolling
fixes the actual cause — rows that are not on screen should not exist — without
changing what the user sees, so it is the only step worth building.

**Change:** a new `src/components/ProcessTable.vue` replaces `v-data-table`.

- Rows are a fixed 36 px and the header 40 px (Vuetify's `density="compact"`
  metrics), so the window arithmetic is exact rather than measured: `startIndex`
  and `endIndex` come from `scrollTop`, and only that slice is rendered, plus 6
  rows of overscan above and below.
- Sorting moves into the component, one column at a time. CPU, memory and priority
  start descending, PID and name ascending; unreadable priorities (`-`) sort last
  in both directions; ties break on PID so the order is stable.
- The same six columns, the tree indentation, the expand chevron, the right-click
  menu, the priority colours and the per-CCD affinity swatches are unchanged. The
  header borrows VTable's `--fixed-header` background and inset shadow, and rows
  borrow its hover background, so the swap is not visible.
- The name tooltip stays a `v-tooltip`: at ~22 live rows its cost is irrelevant,
  and it renders the executable path better than a native `title`.
- The tree-mode child count comes from a `Map` built once per list change
  (`countChildrenByPid`) instead of `countChildren`'s scan of the whole process
  list per rendered row.

**Acceptance:** a 10 s DevTools Performance recording while streaming, 388
processes, flat and tree mode: report scripting + rendering per frame before and
after. Sorting, tree expansion, and the right-click menu must all still work.

**Risk:** a rewrite of the table, so the whole surface has to be re-checked by
hand — but it is confined to one component and the columns cannot drift, since
they are declared in one place.

### O7 — Align the three timers

**Now:** three independent, phase-free timers — the Rust metrics stream (1 s), the
per-core usage poll (`setInterval(..., 1000)` in `App.vue`), and the ProBalance poll
(`setInterval(poll, 2000)` in `ProBalancePanel.vue`). Their bursts drift into the
same frame.

**Change:** keep one JS ticker. `ProBalancePanel` exposes `poll()` through
`defineExpose`; `App.vue` keeps its 1 s core-usage interval and calls
`pbPanelRef.value?.poll()` on every second tick. Remove the panel's internal
interval; the existing `modelValue` watch still starts/stops polling with the panel.

**Acceptance:** with the panel open, the IPC bursts are 1 s apart and aligned with
the core-usage poll; the panel still updates every 2 s; closing the panel still
stops polling.

**Risk:** low. **Value:** minor — do it last, or fold it into the O6 PR.

## 3. Suggested PR batches

| PR | Items | Why together | Gate |
|---|---|---|---|
| 1 | O1 + O5 | Pure back end, no semantic change, and they move the baseline every later measurement is taken against | `cargo check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, 10 s re-measure |
| 2 | O4 | Highest value, needs the new pure-function tests | `cargo test -p cpum-core`, manual "apply twice ⇒ no writes" check, `--force` hatch |
| 3 | O2 + O3 | Both caches; both need the same staleness review | Same as PR 1 + PID-reuse guard test + 2 s priority-freshness check |
| 4 | O6 (virtual scrolling) | Front end only | `npx vue-tsc --noEmit`, `npm run lint:i18n`, `npm run build`, DevTools recording |
| 5 | O7 | Cleanup | `npx vue-tsc --noEmit`, `npm run lint:i18n` |

Cross-cutting for every PR: update `PERFORMANCE.md` numbers, add a `CHANGELOG.md`
`[Unreleased]` line, and keep CI green (Rust, frontend, bundle, audit, secret-scan).

## 4. Re-measurement protocol

Costs scale with process count, so re-measure rather than quoting old numbers.

1. **Service:** the PowerShell snippet in `PERFORMANCE.md` — the raw
   `PercentProcessorTime` delta over a 10 s window.
2. **Sampling round:** the `ctypes` replay of the syscall sequence, recording the
   per-stage table before and after each PR.
3. **Front end:** DevTools Performance, 10 s of streaming, 388 processes, flat and
   tree mode.
4. **Write it down:** record the process count at measurement time next to each
   number, update `PERFORMANCE.md` in the same PR, and mark the corresponding
   backlog row done.

## 5. Decisions (settled)

1. **O3 cadence: 2 s**, as proposed. The priority column is at most 2 s stale; a
   full refresh and a newly-seen process are always exact.
2. **O4 toast count: `applied + skipped`**, as proposed. Pressing "Apply Rules"
   twice reports the processes it verified rather than "0 processes", which reads
   as a bug even though it is the honest answer.
3. **O6: virtual scrolling (step 3), not the incremental steps.** Step 1 would take
   ~388 tooltip instances off a table that still renders every row, and step 2
   (pagination) trades away UX for a partial fix; virtual scrolling removes the
   cause — off-screen rows — and keeps the table looking exactly as it does now.
   See [§2 O6](#o6--cheaper-process-table-rows-front-end).
4. **Where the audit lives:** `PERFORMANCE.md` is untracked and only present in
   the main worktree (`C:/Users/eason/rust/cpum/.workbuddy/`). It stays that way:
   it is a working document whose numbers are re-measured per machine, and it is
   referred to by name only — never linked — from anything that is committed.

## 7. Implementation status

O1, O2, O3, O4, O5, O6 and O7 are implemented.

What landed, in the shape described by §2:

- **O1** - `net_probe::read_network_io_with_handle` is the single read path; the
  sampler passes the handle it already owns. The pid-based `read_network_io` is
  gone (nothing else called it once the sampler stopped).
- **O5** - `enumerate_with_snapshots` walks inline unless `raw.len() >= 600`
  (`PARALLEL_THRESHOLD`); the redundant final sort only runs on the parallel
  branch.
- **O4** - `procwin::affinity_matches` (hard mask, per-group masks, or CPU Sets
  plus an unconstrained hard mask for soft rules) and the pure
  `engine::priorities_match` decide whether `apply_one` writes. `ApplyReport` grew
  a `skipped` count; `apply_affinity_rules` reports `applied + skipped` so a second
  press of "Apply Rules" does not read as "0 processes". Four unit tests cover the
  comparison logic, and `cpum_service --apply-once --force` bypasses it.
- **O2** - `EXE_PATH_CACHE` maps PID to `(creation time, path)`; the creation time
  comes from the `FILETIME` `GetProcessTimes` already fills in.
- **O3** - `PRIORITY_CACHE` with a 2 s window. New PIDs are always read on first
  sight; `list_processes` passes `force = true`.
- **O7** - `ProBalancePanel` exposes `poll()`; `App.vue` calls it on every second
  tick of the core-usage interval, which is now the only front-end timer.
- **O6** - `ProcessTable.vue` renders only the viewport slice (fixed 36 px rows,
  40 px header, 6 rows of overscan) and sorts in-component. `v-data-table` and the
  `:deep(.v-data-table …)` CSS are gone from `App.vue`; the affinity swatch CSS
  moved with the markup. `countChildrenByPid` replaces `countChildren`. Verified
  by an SSR render of the component: 390 rows of input produce 22 rendered rows, a
  14 040 px canvas and a CPU-descending row order.

Still to do before `PERFORMANCE.md` can be updated with real numbers:

1. Service cost - the PowerShell snippet (10 s window).
2. Sampling round - the `ctypes` replay, per-stage table before/after.
3. Front end - a 10 s DevTools recording (this is what O6 is waiting for).
4. Behavioural checks for O4: apply twice and confirm zero writes; change a mask
   by hand and confirm the next tick writes it back; confirm a protected process
   is still attempted every tick.

## 8. Not planned

- No new dependencies (no rayon — the audit shows the work is not parallelisable at
  this scale).
- No change to the 1 s metrics cadence or to the wave/diff event protocol; the
  4-wave design is what keeps the table from flickering and is working.
- No telemetry of any kind. Every number in `PERFORMANCE.md` is produced by a
  script the reader can run, and it stays that way.
- No change to the 5 s rule-polling interval itself. Idempotent application (O4)
  makes the poll cheap; event-driven application is a separate roadmap item in
  `docs/ROADMAP.md`.
