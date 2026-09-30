//! Resolve the data directory the Windows service daemon has to use.
//!
//! Rules, the ProBalance config / status / action journal and the IPC bridge
//! token all live in one directory that the GUI and the service must agree on.
//! Getting it wrong is silent rather than loud: the service keeps running and
//! keeps applying rules, but from a stale directory, so the GUI reports the
//! service as offline (no status heartbeat arrives), the GUI's rule edits
//! appear to be ignored, and the privileged bridge rejects every request
//! (token mismatch) and falls back to a UAC prompt.
//!
//! Windows offers **two independent argument channels** and they are easy to
//! confuse:
//!
//! * Arguments passed by the caller of `StartService` - `sc start <name>
//!   <args>` or the "Start parameters" box in services.msc - are handed to
//!   `ServiceMain` as `lpServiceArgVectors` (index 0 is the service name).
//! * Arguments embedded in the service `ImagePath` - `sc create/config binPath=
//!   "<exe>" "<dir>"`, which is what the app's install command writes - belong
//!   to the service process **command line**. The SCM never forwards them to
//!   `ServiceMain`; they are only visible through `std::env::args()`.
//!
//! [`resolve`] therefore checks both channels before giving up on the legacy
//! machine-level directory (reached only by a manually created service that
//! carries no argument at all).

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// Legacy machine-level data directory (`%ProgramData%\cpum`), used by releases
/// that predate the per-user directory.
const LEGACY_DIR: &str = r"C:\ProgramData\cpum";

/// Last-resort data directory (see [`LEGACY_DIR`]).
pub fn legacy_dir() -> PathBuf {
    PathBuf::from(LEGACY_DIR)
}

/// Whether an argument may be a directory: a flag (`--apply-once`) or an empty
/// string never is.
fn looks_like_dir(argument: &OsStr) -> bool {
    let text = argument.to_string_lossy();
    !text.is_empty() && !text.starts_with('-')
}

/// Data directory from `ServiceMain`'s `lpServiceArgVectors`: index 0 is the
/// service name, so an argument the caller passed to `StartService` follows it.
pub fn from_service_main_args(arguments: &[OsString]) -> Option<PathBuf> {
    arguments
        .iter()
        .skip(1)
        .find(|argument| looks_like_dir(argument))
        .map(PathBuf::from)
}

/// Data directory from the process command line, i.e. the `ImagePath`
/// arguments of the installed service (`argv[0]` is the executable path).
pub fn from_command_line(arguments: &[String]) -> Option<PathBuf> {
    arguments
        .iter()
        .skip(1)
        .find(|argument| looks_like_dir(OsStr::new(argument)))
        .map(PathBuf::from)
}

/// Resolve the data directory. An explicit per-start argument wins over the
/// install-time `ImagePath` argument; the legacy directory is the fallback for
/// a service created without any argument (for example by hand with `sc.exe`).
pub fn resolve(service_main_args: &[OsString], command_line_args: &[String]) -> PathBuf {
    from_service_main_args(service_main_args)
        .or_else(|| from_command_line(command_line_args))
        .unwrap_or_else(legacy_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_DIR: &str = r"C:\Users\user\AppData\Roaming\com.open-nexa.cpum";
    const EXE: &str = r"C:\Users\user\AppData\Local\CPU Manager\cpum_service.exe";

    fn service_main(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn command_line(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn service_main_argument_wins_over_the_image_path() {
        let resolved = resolve(
            &service_main(&["CpumAffinityService", r"D:\explicit"]),
            &command_line(&[EXE, APP_DIR]),
        );
        assert_eq!(resolved, PathBuf::from(r"D:\explicit"));
    }

    /// Regression: the SCM starts the service from `ImagePath` = `"<exe>"
    /// "<dir>"` and passes only the service name to `ServiceMain`, so the
    /// command line is the channel that actually carries the installer-provided
    /// directory. Ignoring it made the service fall back to `%ProgramData%`
    /// while the GUI read `%APPDATA%` - which showed up as "service offline".
    #[test]
    fn image_path_argument_is_used_when_service_main_is_empty() {
        let resolved = resolve(
            &service_main(&["CpumAffinityService"]),
            &command_line(&[EXE, APP_DIR]),
        );
        assert_eq!(resolved, PathBuf::from(APP_DIR));
    }

    #[test]
    fn executable_path_is_never_mistaken_for_the_data_dir() {
        assert_eq!(
            resolve(
                &service_main(&["CpumAffinityService"]),
                &command_line(&[EXE])
            ),
            legacy_dir()
        );
    }

    #[test]
    fn flags_are_not_mistaken_for_directories() {
        assert_eq!(
            resolve(
                &service_main(&["CpumAffinityService", "--apply-once"]),
                &command_line(&[EXE, "--apply-once"])
            ),
            legacy_dir()
        );
    }

    #[test]
    fn legacy_dir_is_the_last_resort() {
        assert_eq!(resolve(&[], &[]), legacy_dir());
    }
}
