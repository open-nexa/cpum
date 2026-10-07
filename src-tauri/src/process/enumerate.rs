//! Process enumeration: ToolHelp fast scan + handle walk (affinity / three
//! priority classes / metrics snapshots).
//!
//! Three paths, increasing cost:
//!  - [`list_processes_light`]: pure fast scan, <20ms, populates the first
//!    frame immediately.
//!  - [`list_processes`]: full scan (fast scan + OpenProcess + differential
//!    rates), backfills the first frame in the background.
//!  - [`enumerate_with_snapshots`]: intermediate layer shared with the
//!    metrics stream (does not assemble Vec<ProcessInfo>).
//!
//! The per-process walk is single-threaded by default: the work is ~26 us of
//! syscalls per process, so at a few hundred processes spawning threads costs
//! more than it saves (audit item O5 measured 11.3 ms with 8 threads vs 10.1 ms
//! single-threaded). Slow-changing fields are cached instead - see
//! [`EXE_PATH_CACHE`] (O2) and [`PRIORITY_CACHE`] (O3).

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WIN32_ERROR};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

use crate::models::ProcessInfo;

// Affinity / CPU Sets / three priority classes are all read/written in
// cpum-core (shared single implementation with the service); here we only
// reuse the handle-related read helpers.
use cpum_core::procwin::{
    mask_to_hex, query_image_path_from_handle, read_affinity_with_handle,
    read_priorities_with_handle, ProcessPriorities,
};

use super::sampling::{
    close_handle, get_number_of_processors, open_handle_for_stats, read_metrics_for_handle,
    RateSample, MIN_SAMPLE_INTERVAL_MS, RATE_CACHE, SNAPSHOT_CACHE,
};

// ---------- Enumeration structures ----------

pub(super) struct ProcessBase {
    pub(super) pid: u32,
    pub(super) name: String,
    /// Full executable path (only resolved during full enumeration; None for
    /// the light fast scan or protected processes).
    pub(super) exe_path: Option<String>,
    pub(super) affinity_mask: Option<String>,
    pub(super) system_affinity_mask: Option<String>,
    pub(super) group_affinity_masks: Option<Vec<String>>,
    pub(super) group_system_affinity_masks: Option<Vec<String>>,
    pub(super) parent_pid: u32,
    pub(super) access_denied: bool,
    pub(super) memory_bytes: u64,
    pub(super) priorities: ProcessPriorities,
}

/// Entry returned by the lightweight fast scan: only PID / name / parent_pid,
/// no OpenProcess call (<20ms).
#[derive(Clone, Debug)]
pub(super) struct RawEntry {
    pub(super) pid: u32,
    pub(super) name: String,
    pub(super) parent_pid: u32,
}

// ---------- ToolHelp fast scan ----------

/// Pure ToolHelp SNAPPROCESS fast scan - only PID / name / parent_pid.
/// Performs no OpenProcess / NtQuery calls -> on the order of 10ms.
pub(super) fn list_basic_entries() -> Result<Vec<RawEntry>, String> {
    let pe32_size = size_of::<PROCESSENTRY32W>() as u32;
    let mut raw: Vec<RawEntry> = Vec::with_capacity(512);

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| format!("CreateToolhelp32Snapshot failed: {}", e))?;

        let mut entry = PROCESSENTRY32W {
            dwSize: pe32_size,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                entry.dwSize = pe32_size;
                raw.push(RawEntry {
                    pid: entry.th32ProcessID,
                    name: pcwstr_to_string(&entry.szExeFile),
                    parent_pid: entry.th32ParentProcessID,
                });

                entry.dwSize = pe32_size;
                match Process32NextW(snapshot, &mut entry) {
                    Ok(()) => continue,
                    Err(e) => {
                        let last_err = WIN32_ERROR::from_error(&e).map(|x| x.0).unwrap_or(0);
                        if last_err == 18 {
                            break;
                        } // ERROR_NO_MORE_FILES
                        let mut next_ok = false;
                        for _ in 0..5 {
                            entry.dwSize = pe32_size;
                            match Process32NextW(snapshot, &mut entry) {
                                Ok(()) => {
                                    next_ok = true;
                                    break;
                                }
                                Err(e2) => {
                                    let err2 =
                                        WIN32_ERROR::from_error(&e2).map(|x| x.0).unwrap_or(0);
                                    if err2 == 18 {
                                        break;
                                    }
                                }
                            }
                        }
                        if next_ok {
                            continue;
                        }
                        break;
                    }
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }

    if !raw.iter().any(|r| r.pid == 0) {
        raw.insert(
            0,
            RawEntry {
                pid: 0,
                name: "System Idle Process".to_string(),
                parent_pid: 0,
            },
        );
    }

    Ok(raw)
}

// ---------- Full enumeration ----------

/// Process count above which the per-process walk is split across threads.
///
/// The per-process work is ~26 us of syscalls, so thread spawn plus
/// synchronisation only pays off on very large process counts (audit item O5).
const PARALLEL_THRESHOLD: usize = 600;

/// How often the three priority classes are re-read from the OS (audit item
/// O3). They change rarely - a rule, ProBalance or the user's own edit - and
/// the metrics diff already surfaces changes, so a 2 s cadence keeps the
/// column fresh while removing ~1.9 ms from every one-second round.
const PRIORITY_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Resolved exe paths, keyed by PID and guarded by the process creation time.
///
/// The path is immutable for the lifetime of a process, but a PID *can* be
/// recycled, so the entry is only used when the creation time still matches
/// (audit item O2). Entries whose process is gone are pruned every round.
static EXE_PATH_CACHE: Lazy<Mutex<HashMap<u32, (u64, String)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Last-read priority classes per PID, plus the time of the last full refresh
/// wave (audit item O3).
static PRIORITY_CACHE: Lazy<Mutex<PriorityCache>> = Lazy::new(|| {
    Mutex::new(PriorityCache {
        values: HashMap::new(),
        last_refresh: None,
    })
});

struct PriorityCache {
    /// Keyed by PID, with the process creation time the entry was read from.
    ///
    /// Unlike the exe path, priorities *can* change - that is what the 2 s
    /// refresh wave is for - but a PID can also be recycled, and a recycled PID
    /// is "alive" as far as `prune_caches` is concerned. Without the creation
    /// time, a new process would be served its predecessor's priority classes
    /// until the next refresh wave. Same rule as `EXE_PATH_CACHE`.
    values: HashMap<u32, (u64, ProcessPriorities)>,
    /// `None` = never refreshed, so the first round always reads.
    last_refresh: Option<Instant>,
}

/// Whether this round must re-read the priority classes of every process.
///
/// `force` is used by the full-refresh path, where the caller needs exact
/// values rather than "at most 2 s old".
fn priority_refresh_due(force: bool) -> bool {
    if force {
        return true;
    }
    match PRIORITY_CACHE.lock() {
        Ok(cache) => match cache.last_refresh {
            None => true,
            Some(at) => at.elapsed() >= PRIORITY_REFRESH_INTERVAL,
        },
        // A poisoned mutex means a previous sampler panicked; fall back to
        // reading everything rather than serving possibly-stale priorities.
        Err(_) => true,
    }
}

fn priority_cache_read(
    pid: u32,
    create_time: Option<u64>,
    handle: HANDLE,
    refresh_due: bool,
) -> ProcessPriorities {
    // A process we have never seen has no cached value, so it is read on the
    // first round regardless of the refresh cadence - a new process must show
    // correct priorities immediately.
    //
    // Without a creation time there is no way to tell a recycled PID from the
    // original one, so neither read from nor write to the cache.
    if !refresh_due {
        if let (Some(create_time), Ok(cache)) = (create_time, PRIORITY_CACHE.lock()) {
            if let Some((cached_time, p)) = cache.values.get(&pid).copied() {
                if cached_time == create_time {
                    return p;
                }
            }
        }
    }
    let priorities = read_priorities_with_handle(handle);
    if let (Some(create_time), Ok(mut cache)) = (create_time, PRIORITY_CACHE.lock()) {
        cache.values.insert(pid, (create_time, priorities));
    }
    priorities
}

/// Resolve the exe path, reusing the cached value when the process is still
/// the same process (`create_time` unchanged).
fn exe_path_cached(pid: u32, create_time: Option<u64>, handle: HANDLE) -> Option<String> {
    // Without a creation time there is no way to tell a recycled PID from the
    // original one, so neither read from nor write to the cache.
    let Some(create_time) = create_time else {
        return query_image_path_from_handle(handle);
    };
    if let Ok(cache) = EXE_PATH_CACHE.lock() {
        if let Some((cached_time, path)) = cache.get(&pid) {
            if *cached_time == create_time {
                return Some(path.clone());
            }
        }
    }
    let path = query_image_path_from_handle(handle)?;
    if let Ok(mut cache) = EXE_PATH_CACHE.lock() {
        cache.insert(pid, (create_time, path.clone()));
    }
    Some(path)
}

/// Drop cache entries for processes that no longer exist. Both caches are
/// keyed by PID, so without this they would grow for the lifetime of the app
/// (and a recycled PID could be served a dead process's path).
fn prune_caches(alive: &HashSet<u32>) {
    if let Ok(mut cache) = EXE_PATH_CACHE.lock() {
        cache.retain(|pid, _| alive.contains(pid));
    }
    if let Ok(mut cache) = PRIORITY_CACHE.lock() {
        cache.values.retain(|pid, _| alive.contains(pid));
    }
}

/// Open one process and read everything the caller needs from it.
fn sample_entry(
    entry: &RawEntry,
    priority_refresh_due: bool,
) -> (ProcessBase, super::sampling::ProcessSnapshot) {
    match open_handle_for_stats(entry.pid) {
        (None, _) => (
            ProcessBase {
                pid: entry.pid,
                name: entry.name.clone(),
                exe_path: None,
                affinity_mask: None,
                system_affinity_mask: None,
                group_affinity_masks: None,
                group_system_affinity_masks: None,
                parent_pid: entry.parent_pid,
                access_denied: true,
                memory_bytes: 0,
                priorities: ProcessPriorities::default(),
            },
            super::sampling::ProcessSnapshot::default(),
        ),
        (Some(h), partially_denied) => {
            let (pm, sm) = read_affinity_with_handle(h);
            let m = read_metrics_for_handle(h);
            let priorities = priority_cache_read(entry.pid, m.create_time, h, priority_refresh_due);
            let exe_path = exe_path_cached(entry.pid, m.create_time, h);
            close_handle(h);
            let snap = super::sampling::ProcessSnapshot {
                cpu_total_ticks: m.cpu_total_ticks.unwrap_or(0),
                disk_read_bytes: m.disk_read_bytes.unwrap_or(0),
                disk_write_bytes: m.disk_write_bytes.unwrap_or(0),
                net_in_bytes: m.net_in_bytes.unwrap_or(0),
                net_out_bytes: m.net_out_bytes.unwrap_or(0),
            };
            let denied = partially_denied || (pm.is_none() && m.cpu_total_ticks.is_none());
            (
                ProcessBase {
                    pid: entry.pid,
                    name: entry.name.clone(),
                    exe_path,
                    affinity_mask: pm.map(mask_to_hex),
                    system_affinity_mask: sm.map(mask_to_hex),
                    group_affinity_masks: None,
                    group_system_affinity_masks: None,
                    parent_pid: entry.parent_pid,
                    access_denied: denied,
                    memory_bytes: m.working_set_bytes,
                    priorities,
                },
                snap,
            )
        }
    }
}

/// Full enumeration: ToolHelp scan + one handle walk per process.
///
/// `force_priority_read` bypasses the 2 s priority cache and re-reads the three
/// priority classes for every process. The metrics stream passes `false` (it
/// runs every second); `list_processes` passes `true`, because a user-triggered
/// full refresh must show exact values.
pub(super) fn enumerate_with_snapshots(
    force_priority_read: bool,
) -> Result<
    (
        Vec<ProcessBase>,
        HashMap<u32, super::sampling::ProcessSnapshot>,
    ),
    String,
> {
    let raw = list_basic_entries()?;
    let refresh_due = priority_refresh_due(force_priority_read);

    let mut all_results: Vec<(usize, ProcessBase, super::sampling::ProcessSnapshot)> =
        Vec::with_capacity(raw.len());

    if raw.len() >= PARALLEL_THRESHOLD {
        // Very large process counts only: the syscall-level concurrency is
        // worth the thread spawns here (audit item O5).
        let nproc = get_number_of_processors() as usize;
        let n_threads = nproc.clamp(2, 8);
        let chunk_size = raw.len().div_ceil(n_threads);

        std::thread::scope(|s| {
            let handles: Vec<
                std::thread::ScopedJoinHandle<
                    '_,
                    Vec<(usize, ProcessBase, super::sampling::ProcessSnapshot)>,
                >,
            > = raw
                .chunks(chunk_size)
                .enumerate()
                .map(|(chunk_idx, chunk)| {
                    let chunk_start = chunk_idx * chunk_size;
                    s.spawn(move || {
                        chunk
                            .iter()
                            .enumerate()
                            .map(|(i, r)| {
                                let (base, snap) = sample_entry(r, refresh_due);
                                (chunk_start + i, base, snap)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();

            for h in handles {
                if let Ok(v) = h.join() {
                    all_results.extend(v);
                }
            }
        });

        // Restore the original order (parallel chunk processing may reorder
        // entries).
        all_results.sort_by_key(|(i, _, _)| *i);
    } else {
        for (i, r) in raw.iter().enumerate() {
            let (base, snap) = sample_entry(r, refresh_due);
            all_results.push((i, base, snap));
        }
    }

    // A refresh wave happened: start the next 2 s window from here. Reading a
    // handful of newly-appeared processes does not reset the window, otherwise
    // a machine that constantly spawns processes would never refresh the rest.
    if refresh_due {
        if let Ok(mut cache) = PRIORITY_CACHE.lock() {
            cache.last_refresh = Some(Instant::now());
        }
    }
    prune_caches(&raw.iter().map(|r| r.pid).collect());

    let group_masks = if cpum_core::procwin::active_group_count() > 1 {
        cpum_core::procwin::aggregate_group_affinity_by_pid().ok()
    } else {
        None
    };
    let group_system_masks = if cpum_core::procwin::active_group_count() > 1 {
        Some(
            (0..cpum_core::procwin::active_group_count())
                .map(|group| {
                    let count = cpum_core::procwin::active_processor_count(group);
                    let mask = if count >= 64 {
                        u64::MAX
                    } else {
                        (1u64 << count) - 1
                    };
                    mask_to_hex(mask)
                })
                .collect(),
        )
    } else {
        None
    };
    let mut bases: Vec<ProcessBase> = Vec::with_capacity(all_results.len());
    let mut snaps: HashMap<u32, super::sampling::ProcessSnapshot> =
        HashMap::with_capacity(all_results.len());
    for (_, mut base, snap) in all_results {
        base.group_affinity_masks = group_masks
            .as_ref()
            .and_then(|m| m.get(&base.pid))
            .map(|masks| masks.iter().map(|mask| mask_to_hex(*mask)).collect());
        base.group_system_affinity_masks = group_system_masks.clone();
        snaps.insert(base.pid, snap);
        bases.push(base);
    }

    Ok((bases, snaps))
}

// ---------- Public API: light list / full list ----------

/// Ultra-lightweight process list: only PID / name / parent_pid, no
/// OpenProcess at all. Returns in <20ms, used to populate the first frame
/// immediately; slow fields (memory / affinity / rates) are patched in
/// later.
pub fn list_processes_light() -> Result<Vec<ProcessInfo>, String> {
    let raw = list_basic_entries()?;
    let mut out: Vec<ProcessInfo> = Vec::with_capacity(raw.len());
    for r in raw {
        out.push(ProcessInfo {
            pid: r.pid,
            name: r.name,
            exe_path: None, // Light scan skips OpenProcess, so no exe path.
            affinity_mask: None,
            system_affinity_mask: None,
            group_affinity_masks: None,
            group_system_affinity_masks: None,
            parent_pid: r.parent_pid,
            access_denied: false, // Unknown: assume we have access; the full
            // enumeration patch will overwrite.
            priority_class: None, // Light scan does not read priorities; the
            // full enumeration / metrics diff will
            // backfill these.
            io_priority: None,
            memory_priority: None,
            cpu_usage_percent: 0.0,
            memory_bytes: 0,
            disk_read_bps: 0,
            disk_write_bps: 0,
            net_in_bps: 0,
            net_out_bps: 0,
        });
    }
    out.sort_by(|a, b| system_process_first(a.pid, b.pid));
    Ok(out)
}

pub fn list_processes() -> Result<Vec<ProcessInfo>, String> {
    let now = Instant::now();
    let nproc = get_number_of_processors() as f32;

    // 1. Fetch the previous cache (may be None = first call).
    let prev_opt: Option<(Instant, HashMap<u32, super::sampling::ProcessSnapshot>)> = {
        let guard = SNAPSHOT_CACHE.lock().map_err(|e| e.to_string())?;
        guard.clone()
    };
    let (prev_time, prev_map) = prev_opt.clone().unwrap_or_else(|| (now, HashMap::new()));
    let dt_ms = now.saturating_duration_since(prev_time).as_millis();

    // 2. Enumerate all processes + collect the current snapshot.
    //    `force_priority_read = true`: a full refresh is either the first frame
    //    or a user-triggered reload, and must not serve a cached priority.
    let (processes_base, snap_map) = enumerate_with_snapshots(true)?;

    // 3. Whenever the interval is >= threshold, compute rates once. (On the
    // first call prev_map is empty, so all rates remain 0 - this is
    // expected; it's used as a baseline anchor.)
    let mut rate_guard = RATE_CACHE.lock().map_err(|e| e.to_string())?;
    if dt_ms >= MIN_SAMPLE_INTERVAL_MS {
        let dt_sec = (dt_ms as f64) / 1000.0;
        for (pid, snap) in snap_map.iter() {
            let prev = prev_map.get(pid).copied();

            // Disk delta
            let (disk_rb, disk_wb) = match prev {
                Some(p) => (
                    snap.disk_read_bytes.saturating_sub(p.disk_read_bytes),
                    snap.disk_write_bytes.saturating_sub(p.disk_write_bytes),
                ),
                None => (0, 0),
            };

            // Net delta (BytesIn / BytesOut, Win11 24H2+; older versions
            // keep the snap fields at 0).
            let (net_in_d, net_out_d) = match prev {
                Some(p) => (
                    snap.net_in_bytes.saturating_sub(p.net_in_bytes),
                    snap.net_out_bytes.saturating_sub(p.net_out_bytes),
                ),
                None => (0, 0),
            };

            // CPU %
            let cpu_percent = match prev {
                Some(p) => {
                    let delta = snap.cpu_total_ticks.saturating_sub(p.cpu_total_ticks) as f64;
                    let delta_cpu_sec = delta * 1e-7;
                    ((delta_cpu_sec / dt_sec) * 100.0) as f32
                }
                None => 0.0,
            };
            let cpu_percent = cpu_percent.max(0.0).min(nproc * 100.0 * 1.1);

            rate_guard.insert(
                *pid,
                RateSample {
                    cpu_percent,
                    disk_read_bps: (disk_rb as f64 / dt_sec) as u64,
                    disk_write_bps: (disk_wb as f64 / dt_sec) as u64,
                    net_in_bps: (net_in_d as f64 / dt_sec) as u64,
                    net_out_bps: (net_out_d as f64 / dt_sec) as u64,
                },
            );
        }
        rate_guard.retain(|pid, _| snap_map.contains_key(pid));
    } else if prev_opt.is_none() {
        // First call: prune dead PIDs (only keep currently-existing
        // processes).
        rate_guard.retain(|pid, _| snap_map.contains_key(pid));
    }

    // 4. ★Key point★: regardless of whether we triggered rate computation,
    //    write the current snapshot to the cache so the next call has a
    //    baseline to diff against. (Earlier bug: the cache was only written
    //    inside the `if` branch, so the first call never wrote anything and
    //    every subsequent dt_ms stayed at 0.)
    {
        let mut cache = SNAPSHOT_CACHE.lock().map_err(|e| e.to_string())?;
        *cache = Some((now, snap_map));
    }

    // Assemble the final result.
    let mut out: Vec<ProcessInfo> = Vec::with_capacity(processes_base.len());
    for pb in processes_base {
        let rs = rate_guard.get(&pb.pid).copied().unwrap_or_default();
        out.push(ProcessInfo {
            pid: pb.pid,
            name: pb.name,
            exe_path: pb.exe_path,
            affinity_mask: pb.affinity_mask,
            system_affinity_mask: pb.system_affinity_mask,
            group_affinity_masks: pb.group_affinity_masks,
            group_system_affinity_masks: pb.group_system_affinity_masks,
            parent_pid: pb.parent_pid,
            access_denied: pb.access_denied,
            priority_class: pb.priorities.priority_class,
            io_priority: pb.priorities.io_priority,
            memory_priority: pb.priorities.memory_priority,
            cpu_usage_percent: rs.cpu_percent,
            memory_bytes: pb.memory_bytes,
            disk_read_bps: rs.disk_read_bps,
            disk_write_bps: rs.disk_write_bps,
            net_in_bps: rs.net_in_bps,
            net_out_bps: rs.net_out_bps,
        });
    }

    out.sort_by(|a, b| system_process_first(a.pid, b.pid));
    Ok(out)
}

// ---------- Process list cache (faster first frame: the next launch shows
// last session's process list directly, slow fields are 0) ----------

fn processes_cache_path(base_dir: &std::path::Path) -> std::path::PathBuf {
    base_dir.join("processes_cache.json")
}

/// Read the process list cache saved from the previous run. Contains only
/// PID/name/parent_pid with slow fields at 0. Returns None if no cache
/// exists or parsing failed; the caller should fall back to
/// `list_processes_light`.
pub fn load_processes_cache(base_dir: &std::path::Path) -> Option<Vec<ProcessInfo>> {
    let path = processes_cache_path(base_dir);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<Vec<ProcessInfo>>(&raw).ok()
}

/// Write the process list to the cache (basic fields only; slow fields are
/// also stored so they display directly next launch). Failures are silently
/// ignored (the cache is just an optimization).
pub fn save_processes_cache(base_dir: &std::path::Path, processes: &[ProcessInfo]) {
    let _ = std::fs::create_dir_all(base_dir);
    let path = processes_cache_path(base_dir);
    if let Ok(json) = serde_json::to_string(processes) {
        let _ = std::fs::write(&path, json);
    }
}

// ---------- Internal helpers ----------

fn pcwstr_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn is_system_process(pid: u32) -> bool {
    pid == 0 || pid == 4
}

/// Stable sort: system processes (PID 0/4) first, the rest sorted by PID
/// ascending (a less surprising initial ordering for the user).
fn system_process_first(a: u32, b: u32) -> std::cmp::Ordering {
    match (is_system_process(a), is_system_process(b)) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.cmp(&b),
    }
}
