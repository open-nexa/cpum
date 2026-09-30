# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |
| < 0.1.0 | No |

Only the latest `v*` release gets fixes. There is no LTS line.

## Reporting a vulnerability

**Do not open a public issue.** Use
[GitHub private security advisories](https://github.com/open-nexa/cpum/security/advisories/new),
or contact a maintainer directly if you prefer.

Please include what you ran, what you expected and what happened. For anything
touching the service or the privileged bridge, the output of

```powershell
sc.exe qc CpumAffinityService
Get-ChildItem "$env:APPDATA\com.open-nexa.cpum"
```

is usually the fastest way to get to a reproduction. You should get an initial
response within a week.

## The privilege model

This is the part worth reading before you report anything, because a lot of what
looks like a vulnerability is the design.

- **The desktop app is never elevated.** The binary embeds an `asInvoker`
  manifest and the installer is a per-user NSIS package installed under
  `%LOCALAPPDATA%`. Neither installing nor using the app raises UAC.
- **Only Windows service management needs administrator rights.**
  `install_service` runs `sc.exe` elevated; uninstall / start / stop try
  unelevated first and elevate only on access denied.
- **Configuration lives in a user-writable directory**,
  `%APPDATA%\com.open-nexa.cpum`, because an un-elevated GUI cannot write to a
  machine-level location. The service runs as `LocalSystem` and receives that
  directory as an argument baked into its `ImagePath` at install time.
- **The privileged bridge** lets the un-elevated GUI change a process it cannot
  open itself. The GUI sends the request over the named pipe
  `\\.\pipe\cpum-bridge-v1` to the service, which holds `SeDebugPrivilege`. The
  pipe DACL allows SYSTEM and interactive users, but every request must carry a
  random token read from `%APPDATA%\com.open-nexa.cpum\bridge.token` — a file
  only that user (and SYSTEM) can read. A different local user can open the pipe
  but cannot produce a valid token.
- **PPL processes (`csrss`, some anti-virus and anti-cheat) cannot be modified
  at all**, not even by an elevated administrator. Those failures are surfaced
  as-is and are not a bug.

## Known gaps

These are known, and a report that restates one of them is still welcome if it
comes with a concrete exploitation path.

- **The bridge's authorization depends on the ACL of `%APPDATA%`.** The token is
  a bearer secret in a file, not a capability. Anything that can read another
  user's `bridge.token` — an administrator, a process already running as that
  user, or a backup restored with the wrong ownership — can issue requests the
  service will accept. The impact is bounded by what the bridge accepts: a
  service that only changes affinity and priority, never code execution.
- **The service trusts the rule file it is pointed at.** It runs as LocalSystem
  and applies whatever `%APPDATA%\com.open-nexa.cpum\affinity_rules.json`
  contains. A rule file is data, not code, but a local attacker who can write to
  another user's data directory can make the service pin that user's processes.
- **The updater trusts one GitHub endpoint.** `plugins.updater.endpoints` points
  at `open-nexa/cpum` releases; the client verifies each artifact against the
  minisign public key embedded in the binary (`src-tauri/tauri.pubkey`). A
  compromised release on that repository, or a leaked signing key, is not
  something the client can detect. Signing keys are never committed — the
  private key lives in the `TAURI_SIGNING_PRIVATE_KEY` CI secret and
  `src-tauri/.gitignore` excludes `tauri.key` — and `.gitleaks.toml` fails the
  build if one shows up in the tree.
- **Authenticode signing is optional.** When no SignPath credentials are
  configured the release workflow publishes unsigned installers. Releases
  published before signing was configured are unsigned.

## Out of scope

- Changing the affinity or priority of a process you already own. That is the
  product.
- Crashes or data loss caused by an affinity mask that starves a process the
  user configured. Test masks before filing.
- Anything that requires the attacker to already be SYSTEM or an administrator
  on the machine.
