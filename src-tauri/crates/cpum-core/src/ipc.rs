//! Privileged bridge between the un-elevated GUI and the LocalSystem service.
//!
//! The desktop app runs as the invoking user (see the `asInvoker` manifest), so
//! it cannot touch processes that belong to another security context:
//! `OpenProcess` fails with `ERROR_ACCESS_DENIED` because a filtered UAC token
//! does not hold `SeDebugPrivilege`. The optional `CpumAffinityService` does
//! hold it, so the GUI can delegate those operations instead of asking the user
//! for a UAC prompt on every single change.
//!
//! Transport: a message-mode named pipe, one JSON request / one JSON response
//! per connection.
//!
//! # Authorization
//!
//! The pipe DACL is deliberately permissive (SYSTEM full control, read/write
//! for interactive users) because the service has no way to know which user
//! will connect. The real gate is a shared secret:
//!
//! * the GUI creates/reads `<rules dir>/bridge.token` (a random value) - that
//!   directory is the per-user `%APPDATA%\<bundle id>` folder, which only that
//!   user and SYSTEM can read;
//! * every request carries the token and the service compares it against the
//!   file it loaded at startup.
//!
//! A different local user can therefore open the pipe but cannot produce a
//! valid token, so they cannot ask a SYSTEM service to modify processes they do
//! not own. Note that this does **not** stop the machine administrator (they can
//! read anything anyway) and it cannot help with PPL / protected processes,
//! which even SYSTEM may not modify.
//!
//! # Why the pipe name is derived from the token
//!
//! The pipe name is machine-global. A fixed, well-known name can be claimed by
//! any local process *before* the service starts, and whoever owns the name
//! receives the next connection - including the request, and with it the
//! token that authenticates every privileged operation. Deriving the name
//! from the token makes it unguessable, so only a peer that already knows the
//! secret can squat it. The service additionally claims the name with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE` and refuses to serve a name it could not
//! claim exclusively.
//!
//! # Foreground relay
//!
//! `GetForegroundWindow` is session-local, so the Session 0 service cannot see
//! the interactive desktop. The GUI samples it once per second and sends a
//! `ReportForeground` request; the service keeps the last observation in a
//! TTL-bounded [`ForegroundCache`] that the ProBalance tick reads. This is
//! also why ProBalance / Game Mode need the app running even when the service
//! is installed: without a fresh report the engine deliberately treats the
//! foreground as unknown rather than acting on a stale PID.
//!
//! # Shutdown
//!
//! [`serve`] blocks in `ConnectNamedPipe` and is meant to run on a background
//! thread of the service process; it is terminated when the process exits.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use windows::core::{w, HRESULT, HSTRING};
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::engine::{self, ProcessApplyInfo};
use crate::monitor::{ForegroundSnapshot, ForegroundState};
use crate::procwin;
use crate::rule::RuleMode;

/// Name of the shared-secret file inside the per-user rules directory.
const TOKEN_FILE: &str = "bridge.token";
/// Requests and responses are small JSON documents; 64 KiB is plenty.
const MAX_MESSAGE: usize = 64 * 1024;
/// Deadline for one bridge round trip.
///
/// The service answers on the same connection, so a request that is not
/// answered within this window means the service is wedged. The round trip
/// runs on a worker thread, so the deadline is what keeps a wedged service
/// from freezing the caller (and with it the GUI) forever.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// How long the pipe name derived from the token may be, in hex characters
/// (128 bits of the token).
const PIPE_NAME_ENTROPY_CHARS: usize = 32;
/// How long a GUI-reported foreground observation stays usable.
///
/// The GUI reports once per second, so a few missed reports must not make
/// ProBalance flicker; but a stale observation must not outlive the app that
/// produced it. Four seconds tolerates four lost reports and expires quickly
/// once the app closes.
const FOREGROUND_TTL: Duration = Duration::from_secs(4);

// ---------- Wire protocol ----------

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness check; also used to discover whether the bridge is up.
    Ping { token: String },
    /// Apply a set of per-group affinity masks (hex strings, e.g. `"0xFF"`).
    SetAffinity {
        token: String,
        pid: u32,
        masks: Vec<String>,
        mode: RuleMode,
    },
    /// Adjust any combination of the three priority classes.
    SetPriority {
        token: String,
        pid: u32,
        priority_class: Option<u32>,
        io_priority: Option<u32>,
        memory_priority: Option<u32>,
    },
    /// Re-apply every enabled rule using the service's own rules directory.
    ApplyRules { token: String },
    /// Relay the foreground window observed by the GUI. The service runs in
    /// Session 0 and cannot call the session-local Win32 APIs itself, so the
    /// GUI is the "eyes" and the service stays the privileged executor.
    ReportForeground {
        token: String,
        state: ForegroundState,
    },
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    /// Number of successful (process x rule) applications (`apply_rules`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub applied: Option<u32>,
    /// PIDs the service found already in the desired state and therefore did
    /// not write. They are not in `changed` (their rows already show the
    /// right values), so the GUI needs them separately - otherwise a process
    /// the service skipped is missing from the "applied to N processes"
    /// count. Optional so a newer GUI still talks to an older service.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub skipped_pids: Option<Vec<u32>>,
    /// Per-process result of `apply_rules`, so the GUI can patch its rows.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub changed: Option<Vec<AppliedProcess>>,
    /// Priorities read back after `set_priority`. The GUI cannot always read
    /// them itself (reading a SYSTEM process needs the very privilege it just
    /// delegated), so the service returns them.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub priorities: Option<procwin::ProcessPriorities>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppliedProcess {
    pub pid: u32,
    pub mask_hex: Option<String>,
    pub priorities: Option<procwin::ProcessPriorities>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            ..Default::default()
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            ..Default::default()
        }
    }

    pub fn applied(applied: u32, skipped_pids: Vec<u32>, changed: Vec<AppliedProcess>) -> Self {
        Self {
            ok: true,
            applied: Some(applied),
            skipped_pids: Some(skipped_pids),
            changed: Some(changed),
            ..Default::default()
        }
    }
}

impl From<&ProcessApplyInfo> for AppliedProcess {
    fn from(value: &ProcessApplyInfo) -> Self {
        Self {
            pid: value.pid,
            mask_hex: value.mask_hex.clone(),
            priorities: value.priorities,
        }
    }
}

// ---------- Foreground observation cache (service side) ----------

/// Last foreground observation relayed by the GUI, shared between the bridge
/// listener threads and the service's ProBalance tick.
///
/// The TTL is what makes "the app was closed" recoverable: without it the
/// service would keep treating the last foreground PID as current forever,
/// and would keep downgrading background processes on behalf of a window that
/// no longer exists.
pub struct ForegroundCache {
    last: Option<(Instant, ForegroundState)>,
    ttl: Duration,
}

/// Thread-safe handle handed to [`serve`] and read once per ProBalance tick.
pub type SharedForeground = Arc<Mutex<ForegroundCache>>;

impl ForegroundCache {
    pub fn new(ttl: Duration) -> Self {
        Self { last: None, ttl }
    }

    /// Record an observation from an authenticated GUI request.
    pub fn report(&mut self, state: ForegroundState) {
        self.report_at(state, Instant::now());
    }

    fn report_at(&mut self, state: ForegroundState, at: Instant) {
        self.last = Some((at, state));
    }

    /// Current observation. An expired or missing report yields an empty
    /// state with `fresh = false`, which the engine treats as "no foreground"
    /// rather than guessing.
    pub fn snapshot(&self) -> ForegroundSnapshot {
        match self.last {
            Some((at, state)) if at.elapsed() <= self.ttl => {
                ForegroundSnapshot { state, fresh: true }
            }
            _ => ForegroundSnapshot::default(),
        }
    }
}

/// Create the cache shared by the bridge and the ProBalance runtime.
pub fn new_foreground_cache() -> SharedForeground {
    Arc::new(Mutex::new(ForegroundCache::new(FOREGROUND_TTL)))
}

// ---------- Shared secret ----------

/// Return the bridge token for `dir`, creating it on first use.
///
/// Called by the GUI (which owns the directory). The service only reads it.
pub fn ensure_token(dir: &Path) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("failed to create rules directory: {e}"))?;
    let path = dir.join(TOKEN_FILE);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim();
        if !existing.is_empty() {
            return Ok(existing.to_string());
        }
    }
    let token = format!("{}{}", Uuid::new_v4(), Uuid::new_v4());
    // Write atomically and read back: the pipe name is derived from this
    // token, so both ends must agree on the value that ended up on disk.
    crate::store::atomic_write(&path, &token)
        .map_err(|e| format!("failed to write bridge token: {e}"))?;
    let stored = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read back bridge token: {e}"))?;
    let stored = stored.trim();
    if stored.is_empty() {
        return Err("bridge token file is empty after writing".to_string());
    }
    Ok(stored.to_string())
}

/// Read the token without creating it (service side).
pub fn read_token(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(TOKEN_FILE)).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ---------- Handle wrapper ----------

struct Pipe(HANDLE);

// SAFETY: the wrapper owns the handle exclusively. It is created on the
// listener thread and then moved into exactly one connection thread, so no two
// threads ever use it concurrently.
unsafe impl Send for Pipe {}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// ---------- Pipe name ----------

/// The pipe name that belongs to a given token.
///
/// Deriving the name from the secret (instead of using a fixed, well-known
/// one) is what keeps a local process from impersonating the service: the
/// name is machine-global, and whoever owns it receives the next connection -
/// including the token that authenticates every privileged request. A name
/// built from 128 bits of the token cannot be guessed by a process that does
/// not already have the token.
fn pipe_name(token: &str) -> String {
    let entropy: String = token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(PIPE_NAME_ENTROPY_CHARS)
        .collect();
    if entropy.is_empty() {
        // The token is generated by this crate, so this cannot happen - but
        // an empty name would silently degrade to a shared pipe, so refuse.
        return String::new();
    }
    format!(r"\\.\pipe\cpum-bridge-{entropy}")
}

/// The shared secret carried by a request (every variant has one).
fn request_token(request: &Request) -> Result<String, String> {
    let token = match request {
        Request::Ping { token }
        | Request::SetAffinity { token, .. }
        | Request::SetPriority { token, .. }
        | Request::ApplyRules { token }
        | Request::ReportForeground { token, .. } => token,
    };
    if token.trim().is_empty() {
        return Err("bridge token is empty".to_string());
    }
    Ok(token.clone())
}

// ---------- Client (GUI side) ----------

/// Send one request to the service bridge and return its response.
///
/// Returns `Err` when the service is not installed / not running (the pipe does
/// not exist) so the caller can fall back to another strategy.
pub fn request(req: &Request) -> Result<Response, String> {
    let token = request_token(req)?;
    let name = pipe_name(&token);
    if name.is_empty() {
        return Err("bridge token does not yield a usable pipe name".to_string());
    }

    let handle = unsafe {
        CreateFileW(
            &HSTRING::from(name),
            FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .map_err(|e| format!("service bridge unavailable: {e}"))?;

    let payload = serde_json::to_vec(req).map_err(|e| format!("failed to encode request: {e}"))?;

    // The round trip runs on a worker thread with a deadline: a pipe read has
    // no timeout of its own, so a service that never answers would otherwise
    // block the caller - and with it the GUI thread - forever. The worker
    // owns the pipe handle, so it is closed when the request completes.
    let pipe = Pipe(handle);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(round_trip(&pipe, &payload));
    });

    match rx.recv_timeout(REQUEST_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err(format!(
            "bridge request timed out after {} seconds",
            REQUEST_TIMEOUT.as_secs()
        )),
    }
}

/// One write + read exchange on an established connection.
fn round_trip(pipe: &Pipe, payload: &[u8]) -> Result<Response, String> {
    unsafe { WriteFile(pipe.0, Some(payload), None, None) }
        .map_err(|e| format!("bridge write failed: {e}"))?;

    let mut buffer = vec![0u8; MAX_MESSAGE];
    let mut read = 0u32;
    unsafe { ReadFile(pipe.0, Some(&mut buffer[..]), Some(&mut read), None) }
        .map_err(|e| format!("bridge read failed: {e}"))?;

    serde_json::from_slice(&buffer[..read as usize])
        .map_err(|e| format!("bad bridge response: {e}"))
}

// ---------- Server (service side) ----------

/// Build a security descriptor for the pipe from [`PIPE_SDDL`].
///
/// The returned pointer stays allocated for the lifetime of the process (it is
/// intentionally leaked - one descriptor per process).
fn pipe_security_descriptor() -> Option<PSECURITY_DESCRIPTOR> {
    // GA = generic all for SYSTEM, GRGW = read/write for INTERACTIVE (locally
    // logged-on users). See the module docs: the token is the real gate.
    let mut sd = PSECURITY_DESCRIPTOR::default();
    let result = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:(A;;GA;;;SY)(A;;GRGW;;;IU)"),
            1,
            &mut sd,
            None,
        )
    };
    result.ok().map(|()| sd)
}

/// Listen on the pipe and dispatch requests until the process exits.
///
/// `rules_dir` is the directory the service was started with; it is used both
/// to locate the token file and to load rules for the `apply_rules` operation.
pub fn serve(rules_dir: PathBuf, foreground: SharedForeground) {
    // The service process already enabled SeDebugPrivilege, but the helper is
    // cheap and keeps this function usable from any host.
    let _ = procwin::enable_debug_privilege();

    let Some(sd) = pipe_security_descriptor() else {
        eprintln!("cpum bridge: cannot build pipe security descriptor");
        return;
    };
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };

    // The pipe name belongs to the token, so the service needs the token
    // before it can accept a connection. The GUI creates the token when it
    // installs the service, so it normally exists already; wait for it when
    // the service was started by hand.
    let mut owned_name: Option<String> = None;

    loop {
        let token = match read_token(&rules_dir) {
            Some(token) => token,
            None => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        let name = pipe_name(&token);
        if name.is_empty() {
            eprintln!("cpum bridge: the bridge token does not yield a usable pipe name");
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }

        // The first instance of a name claims it exclusively. Without the flag
        // the service would happily share the name with whoever created it
        // first - and that process would receive the next connection,
        // token included. A name we already own must not carry the flag:
        // CreateNamedPipeW would then fail with ERROR_ACCESS_DENIED.
        let claim = owned_name.as_deref() != Some(name.as_str());
        let open_mode = PIPE_ACCESS_DUPLEX
            | if claim {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                FILE_FLAGS_AND_ATTRIBUTES(0)
            };

        let handle = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(name.clone()),
                open_mode,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                MAX_MESSAGE as u32,
                MAX_MESSAGE as u32,
                0,
                Some(&attributes),
            )
        };
        if handle.is_invalid() {
            if claim {
                // Someone else owns the name: fail closed instead of serving
                // (and leaking the token to) an impostor.
                eprintln!(
                    "cpum bridge: the pipe name is already owned by another process; refusing to serve it"
                );
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
            eprintln!("cpum bridge: CreateNamedPipeW failed");
            return;
        }
        owned_name = Some(name);
        let pipe = Pipe(handle);

        // ConnectNamedPipe reports ERROR_PIPE_CONNECTED when the client
        // connected before the server called it. That is a *successful*
        // connection, not a failure: treating it as an error drops the
        // instance, and the client then waits forever for a response that
        // will never arrive.
        if let Err(error) = unsafe { ConnectNamedPipe(pipe.0, None) } {
            if error.code() != HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) {
                // A client that connects and hangs up immediately can make
                // this fail; dropping the pipe instance and looping is the
                // documented recovery.
                continue;
            }
        }

        let dir = rules_dir.clone();
        let fg = foreground.clone();
        std::thread::spawn(move || handle_connection(pipe, &dir, &fg));
    }
}

fn handle_connection(pipe: Pipe, rules_dir: &Path, foreground: &SharedForeground) {
    let mut buffer = vec![0u8; MAX_MESSAGE];
    let mut read = 0u32;

    let response = match unsafe { ReadFile(pipe.0, Some(&mut buffer[..]), Some(&mut read), None) } {
        Ok(()) => match serde_json::from_slice::<Request>(&buffer[..read as usize]) {
            Ok(request) => dispatch(request, rules_dir, foreground),
            Err(e) => Response::error(format!("bad request: {e}")),
        },
        Err(e) => Response::error(format!("read failed: {e}")),
    };

    let payload = serde_json::to_vec(&response).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    let _ = unsafe { WriteFile(pipe.0, Some(&payload), None, None) };
    let _ = unsafe { DisconnectNamedPipe(pipe.0) };
}

fn dispatch(request: Request, rules_dir: &Path, foreground: &SharedForeground) -> Response {
    let token = match &request {
        Request::Ping { token }
        | Request::SetAffinity { token, .. }
        | Request::SetPriority { token, .. }
        | Request::ApplyRules { token }
        | Request::ReportForeground { token, .. } => token,
    };

    match read_token(rules_dir) {
        Some(expected) if &expected == token => {}
        _ => return Response::error("unauthorized: bridge token mismatch"),
    }

    match request {
        Request::Ping { .. } => Response::ok(),
        Request::SetAffinity {
            pid, masks, mode, ..
        } => {
            let masks = match procwin::GroupMasks::from_hex_list(&masks) {
                Ok(masks) => masks.0,
                Err(e) => return Response::error(e),
            };
            match procwin::set_affinity_by_group_masks(pid, &masks, mode) {
                Ok(_) => Response::ok(),
                Err(e) => Response::error(e),
            }
        }
        Request::SetPriority {
            pid,
            priority_class,
            io_priority,
            memory_priority,
            ..
        } => {
            // The bridge is a privileged entry point: validate the values
            // against the same ranges the rule model uses. The undocumented
            // IO priority in particular accepts whatever it is handed.
            if let Some(pc) = priority_class {
                if !crate::rule::valid_priority_class(pc) {
                    return Response::error(format!("invalid CPU priority class: 0x{pc:X}"));
                }
            }
            if let Some(io) = io_priority {
                if !crate::rule::valid_io_priority(io) {
                    return Response::error(format!(
                        "invalid IO priority: {io} (allowed: 0=Very Low, 1=Low, 2=Normal)"
                    ));
                }
            }
            if let Some(mp) = memory_priority {
                if !crate::rule::valid_memory_priority(mp) {
                    return Response::error(format!(
                        "invalid memory priority: {mp} (allowed 1=Very Low ... 5=Normal)"
                    ));
                }
            }
            if let Some(value) = priority_class {
                if let Err(e) = procwin::set_process_priority_class(pid, value) {
                    return Response::error(e);
                }
            }
            if let Some(value) = io_priority {
                if let Err(e) = procwin::set_process_io_priority(pid, value) {
                    return Response::error(e);
                }
            }
            if let Some(value) = memory_priority {
                if let Err(e) = procwin::set_process_memory_priority(pid, value) {
                    return Response::error(e);
                }
            }
            let priorities = procwin::get_process_priorities(pid);
            let mut response = Response::ok();
            response.priorities = Some(priorities);
            response
        }
        Request::ApplyRules { .. } => match engine::apply_rules_from_dir(rules_dir) {
            Ok(report) => Response::applied(
                report.applied,
                report.skipped_pids,
                report.changed.iter().map(AppliedProcess::from).collect(),
            ),
            Err(e) => Response::error(e),
        },
        Request::ReportForeground { state, .. } => match foreground.lock() {
            Ok(mut cache) => {
                cache.report(state);
                Response::ok()
            }
            Err(_) => Response::error("foreground cache is unavailable"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{pipe_name, request_token, ForegroundCache, Request, FOREGROUND_TTL};
    use crate::monitor::ForegroundState;
    use std::time::{Duration, Instant};

    /// The pipe name must be derived from the token: a fixed, well-known name
    /// can be claimed by any local process before the service starts, and that
    /// process would then receive the token out of the next request.
    #[test]
    fn pipe_name_depends_on_the_token() {
        let a = pipe_name("token-a");
        let b = pipe_name("token-b");
        assert_ne!(a, b, "different tokens must yield different pipe names");
        assert!(a.starts_with(r"\\.\pipe\cpum-bridge-"));
        assert!(b.starts_with(r"\\.\pipe\cpum-bridge-"));
    }

    /// The name must stay a valid pipe name and must not leak the token.
    #[test]
    fn pipe_name_is_sanitized_and_truncated() {
        let token = "a1b2-c3d4_e5f6";
        let name = pipe_name(token);
        assert_eq!(name, r"\\.\pipe\cpum-bridge-a1b2c3d4e5f6");

        // Only the entropy part is derived from the token - it must not carry
        // the separators, which are not valid in a pipe name.
        let entropy = name.rsplit('-').next().unwrap();
        assert_eq!(entropy, "a1b2c3d4e5f6");
    }

    /// An empty token must not fall back to a shared, guessable pipe name.
    #[test]
    fn empty_token_yields_no_pipe_name() {
        assert_eq!(pipe_name(""), "");
        assert_eq!(pipe_name("---"), "");
    }

    #[test]
    fn foreground_cache_returns_a_fresh_report() {
        let mut cache = ForegroundCache::new(Duration::from_secs(5));
        let state = ForegroundState {
            pid: Some(4242),
            fullscreen: true,
        };
        cache.report(state);
        let snapshot = cache.snapshot();
        assert!(snapshot.fresh);
        assert_eq!(snapshot.state, state);
    }

    #[test]
    fn foreground_cache_expires_stale_reports() {
        let ttl = Duration::from_secs(4);
        let mut cache = ForegroundCache::new(ttl);
        let state = ForegroundState {
            pid: Some(1),
            fullscreen: false,
        };
        // Just inside the TTL the observation is still usable; just outside
        // it must be discarded, not treated as "no foreground window".
        cache.report_at(state, Instant::now() - ttl + Duration::from_millis(500));
        assert!(cache.snapshot().fresh);
        cache.report_at(state, Instant::now() - ttl - Duration::from_millis(500));
        let snapshot = cache.snapshot();
        assert!(!snapshot.fresh);
        assert_eq!(snapshot.state, ForegroundState::default());
    }

    /// Regression for issue #34: with no GUI report the service must report
    /// "unknown foreground" instead of a fabricated PID.
    #[test]
    fn foreground_cache_without_a_report_is_unknown() {
        let cache = ForegroundCache::new(FOREGROUND_TTL);
        let snapshot = cache.snapshot();
        assert!(!snapshot.fresh);
        assert_eq!(snapshot.state.pid, None);
        assert!(!snapshot.state.fullscreen);
    }

    #[test]
    fn report_foreground_request_carries_the_token_and_state() {
        let request = Request::ReportForeground {
            token: "secret".into(),
            state: ForegroundState {
                pid: Some(99),
                fullscreen: true,
            },
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("report_foreground"));
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(request_token(&decoded).unwrap(), "secret");
        match decoded {
            Request::ReportForeground { state, .. } => {
                assert_eq!(state.pid, Some(99));
                assert!(state.fullscreen);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
