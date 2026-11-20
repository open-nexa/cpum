//! Lightweight monitor: process CPU differential sampling + foreground
//! window observation.
//!
//!  - [`CpuSampler`]: ToolHelp enumeration + GetProcessTimes differential
//!    -> per-process CPU% (single-core baseline), used by the service-side
//!    ProBalance runtime.
//!  - [`get_foreground_state`]: foreground window -> PID + fullscreen state.
//!    These Win32 APIs are **session-local**: they describe the interactive
//!    desktop of the calling thread. A `LocalSystem` service lives in
//!    Session 0, has no interactive windows, and can therefore never observe
//!    the user's foreground window. The GUI, which runs in the interactive
//!    session, samples them once per second and relays the result to the
//!    service over the IPC bridge - see [`crate::ipc::ForegroundCache`].
//!    A foreground *observation* is thus never taken by the service itself.

use std::collections::HashMap;
use std::mem::size_of;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

// =========================================================================
// Process CPU differential sampling
// =========================================================================

/// Convert a CPU-time delta into a single-core-baseline CPU percentage.
///
/// `GetProcessTimes` reports CPU time in **100-nanosecond units**, so the raw
/// tick delta has to be scaled to seconds before it is divided by the
/// wall-clock delta. Skipping that conversion inflates the result by 1e7: a
/// fully busy core would read 1e9 % instead of 100 %.
///
/// This lives here (rather than inline in each sampler) because the GUI's
/// metrics stream and the service-side ProBalance sampler both need it, and
/// the two must agree on the scale - the ProBalance thresholds
/// (`fg_cpu_threshold` = 100 = one full core) are meaningless otherwise.
pub fn cpu_percent_from_ticks(delta_ticks: u64, wall_secs: f64) -> f32 {
    // A non-finite or non-positive interval cannot produce a meaningful
    // percentage (and would poison the thresholds with NaN).
    if !wall_secs.is_finite() || wall_secs <= 0.0 {
        return 0.0;
    }
    let delta_cpu_sec = delta_ticks as f64 * 1e-7;
    ((delta_cpu_sec / wall_secs) * 100.0) as f32
}

/// One process's sample result.
pub struct ProcSample {
    pub pid: u32,
    /// Executable file name (e.g. "codex.exe").
    pub name: String,
    /// CPU usage, single-core baseline: 100.0 = one full logical core,
    /// 8 cores fully used = 800.0.
    pub cpu_percent: f32,
    /// Full executable path (only when the sample request asked to resolve
    /// it and the resolution succeeded; needed by Path-type protection
    /// rules).
    pub image_path: Option<String>,
}

/// Differential CPU sampler for processes.
///
/// Each [`tick`](CpuSampler::tick) enumerates all processes and reads the
/// cumulative CPU time (kernel + user), then diffs against the previous
/// snapshot. The first round only establishes the baseline (all CPUs read
/// 0); rates become available from the second round on. For ~300 processes,
/// the per-round OpenProcess + GetProcessTimes cost is a few milliseconds -
/// well within a 1-second service period.
pub struct CpuSampler {
    last: HashMap<u32, u64>,
    last_tick: Option<Instant>,
}

impl Default for CpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuSampler {
    pub fn new() -> Self {
        Self {
            last: HashMap::new(),
            last_tick: None,
        }
    }

    /// Take one sample; enumeration failure returns Err (caller may ignore
    /// and retry next round). Processes whose handle cannot be opened
    /// (protected / already exited) are absent from the result - the
    /// engine uses this fact to determine liveness.
    ///
    /// When `include_paths=true`, the full path is resolved on the same
    /// handle (needed by Path-type protection rules; each OpenProcess adds
    /// one QueryFullProcessImageNameW call with no extra handle cost).
    /// The common case with no Path rules passes false for zero overhead.
    pub fn tick(&mut self, include_paths: bool) -> Result<Vec<ProcSample>, String> {
        let now = Instant::now();
        let wall_secs = self
            .last_tick
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);

        let mut current: HashMap<u32, u64> = HashMap::new();
        let mut samples: Vec<ProcSample> = Vec::new();

        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                .map_err(|e| format!("CreateToolhelp32Snapshot failed: {e}"))?;
            let mut entry = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let pid = entry.th32ProcessID;
                    if let Some((total, image_path)) = process_sample(pid, include_paths) {
                        let prev = self.last.get(&pid).copied();
                        current.insert(pid, total);
                        // Single-core baseline CPU%: process CPU time delta
                        // / wall-clock delta x 100. The delta is in 100-ns
                        // units and has to be scaled to seconds first - see
                        // `cpu_percent_from_ticks`.
                        let cpu_percent = match prev {
                            Some(p) => cpu_percent_from_ticks(total.saturating_sub(p), wall_secs),
                            None => 0.0,
                        };
                        let name_len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                        let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
                        samples.push(ProcSample {
                            pid,
                            name,
                            cpu_percent,
                            image_path,
                        });
                    }
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
        }

        self.last = current;
        self.last_tick = Some(now);
        Ok(samples)
    }
}

/// Open the process handle and read the cumulative CPU time (kernel + user,
/// 100-ns units). Optionally resolves the full path on the same handle.
/// Returns None when the handle cannot be opened or the process has exited.
fn process_sample(pid: u32, include_path: bool) -> Option<(u64, Option<String>)> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut _create = FILETIME::default();
        let mut _exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let ok = GetProcessTimes(handle, &mut _create, &mut _exit, &mut kernel, &mut user).is_ok();
        let image_path = if ok && include_path {
            query_image_path_with_handle(handle)
        } else {
            None
        };
        let _ = CloseHandle(handle);
        if !ok {
            return None;
        }
        Some((ft_to_u64(kernel) + ft_to_u64(user), image_path))
    }
}

/// Resolve the full executable path using an already-open handle (returns
/// None on failure).
fn query_image_path_with_handle(handle: windows::Win32::Foundation::HANDLE) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
    unsafe {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        if result.is_ok() {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        } else {
            None
        }
    }
}

fn ft_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64 & 0xFFFF_FFFF)
}

// =========================================================================
// Foreground process identification
// =========================================================================

/// Foreground window observation, serialized over the GUI -> service bridge.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForegroundState {
    /// PID owning the foreground window; `None` when there is no foreground
    /// window (lock screen, no interactive session, API failure).
    pub pid: Option<u32>,
    /// Whether the user is presenting a fullscreen Direct3D application.
    pub fullscreen: bool,
}

/// A foreground observation plus whether it is still fresh.
///
/// ProBalance must never act on a stale PID (the app may have exited, or a
/// completely different window may now be in front), so `fresh = false` means
/// "no foreground information" and the engine degrades to idle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForegroundSnapshot {
    pub state: ForegroundState,
    pub fresh: bool,
}

/// Read the foreground process and fullscreen presentation state on the
/// **calling thread's** input desktop.
///
/// `GetForegroundWindow` and `SHQueryUserNotificationState` are session-local:
/// they describe the desktop of the thread that calls them. This function must
/// therefore run in the interactive session - i.e. in the GUI (or a helper
/// process started in that session). A `LocalSystem` service runs in Session 0
/// and cannot observe the user's foreground window; the GUI relays this result
/// to the service via `ipc::Request::ReportForeground` instead.
///
/// There is deliberately no `OpenInputDesktop` / `SetThreadDesktop` here.
/// That "session switch trick" could not bridge Session 0 -> Session 1 in the
/// first place, and a thread that already owns windows cannot switch desktops
/// (`ERROR_BUSY`), so it returned `None` even inside the interactive session.
/// Reading the foreground window directly is both correct and cheaper.
pub fn get_foreground_state() -> ForegroundState {
    unsafe {
        let hwnd = GetForegroundWindow();
        let pid = if hwnd.0.is_null() {
            None
        } else {
            let mut pid = 0u32;
            if GetWindowThreadProcessId(hwnd, Some(&mut pid)) != 0 && pid != 0 {
                Some(pid)
            } else {
                None
            }
        };
        // Any API failure means "not fullscreen": Game Mode must never
        // activate on a guess.
        let fullscreen = SHQueryUserNotificationState()
            .map(|state| state == QUNS_RUNNING_D3D_FULL_SCREEN)
            .unwrap_or(false);
        ForegroundState { pid, fullscreen }
    }
}

/// Current foreground process PID (None if it cannot be determined).
pub fn get_foreground_pid() -> Option<u32> {
    get_foreground_state().pid
}

#[cfg(test)]
mod tests {
    use super::cpu_percent_from_ticks;

    /// Regression: `GetProcessTimes` reports 100-ns units. A process that
    /// consumed one full core over one second must read 100 %, not 1e9 %.
    #[test]
    fn one_full_core_reads_one_hundred_percent() {
        let one_second_in_ticks: u64 = 10_000_000; // 1e7 * 1e-7 s
        let value = cpu_percent_from_ticks(one_second_in_ticks, 1.0);
        assert!((value - 100.0).abs() < 0.01, "expected 100.0, got {value}");
    }

    /// The same core spread over two wall-clock seconds is 50 %.
    #[test]
    fn half_a_core_reads_fifty_percent() {
        let one_second_in_ticks: u64 = 10_000_000;
        let value = cpu_percent_from_ticks(one_second_in_ticks, 2.0);
        assert!((value - 50.0).abs() < 0.01, "expected 50.0, got {value}");
    }

    /// A process that did nothing must read 0 %, and a zero-length interval
    /// must not produce NaN or infinity (it would poison the thresholds).
    #[test]
    fn idle_and_zero_interval_are_zero() {
        assert_eq!(cpu_percent_from_ticks(0, 1.0), 0.0);
        assert_eq!(cpu_percent_from_ticks(1_000_000, 0.0), 0.0);
    }
}
