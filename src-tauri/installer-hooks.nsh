; ---------------------------------------------------------------------------
; CPU Manager NSIS hooks
;
; The installer is a per-user ("currentUser") installer: it runs un-elevated
; and installs into %LOCALAPPDATA%, and the desktop app itself never needs
; administrator rights. Only Windows service management requires elevation,
; so every service command below is skipped when the service is not installed
; and is otherwise re-launched through an elevated helper (one UAC prompt).
;
; Rules live in the per-user Tauri data directory
; (%APPDATA%\com.open-nexa.cpum) because an un-elevated app cannot write to
; machine-level locations such as
; %ProgramData%. The service receives that directory as its startup argument
; and runs as LocalSystem, which can read it.
;
; Surviving an upgrade
; --------------------
; Installing a new version over an existing one does not always replace the
; files in place: the reinstall page Tauri shows for an upgrade defaults to
; "uninstall before installing", which runs the *previous* version's
; uninstaller first. That uninstaller removes the service (it must, otherwise
; a real uninstall would leave a service pointing at a deleted binary), which
; would silently drop a service the user had installed.
;
; The two hooks below work around that with a marker file
; (%APPDATA%\com.open-nexa.cpum\service_installed.flag, see CPUM_SERVICE_FLAG):
;
;   * PREUNINSTALL records the marker before deleting the service when it is
;     running as part of an upgrade, and removes it on a real uninstall.
;   * POSTINSTALL restores the service (sc create + start) when the marker is
;     there but the service is gone, and refreshes the marker when the service
;     is still installed.
;
; The same file is written by the app when the user installs or removes the
; service (see SERVICE_FLAG_FILE in src/lib.rs), so the marker always reflects
; what the user asked for rather than what just happened to be registered.
; ---------------------------------------------------------------------------

!ifndef CPUM_SERVICE_NAME
  !define CPUM_SERVICE_NAME "CpumAffinityService"
!endif

; Mirrors SERVICE_FLAG_FILE in src/lib.rs. Kept as a literal because the
; bundle identifier is not defined yet at the point this file is included.
!ifndef CPUM_SERVICE_FLAG
  !define CPUM_SERVICE_FLAG "$APPDATA\com.open-nexa.cpum\service_installed.flag"
!endif

; Where the marker lived before the bundle identifier changed. An upgrade from
; one of those builds writes the marker here, so POSTINSTALL has to read both.
!ifndef CPUM_SERVICE_FLAG_PREVIOUS_ID
  !define CPUM_SERVICE_FLAG_PREVIOUS_ID "$APPDATA\com.eason.cpum\service_installed.flag"
!endif

; The data directory of the same builds, used as a migration source below.
!ifndef CPUM_DATA_DIR_PREVIOUS_ID
  !define CPUM_DATA_DIR_PREVIOUS_ID "$APPDATA\com.eason.cpum"
!endif

; Run the cmd.exe command line held in $R1 through an elevated (UAC) cmd.exe
; and wait for it to finish.
;
; Notes on why this is written the way it is:
;   * `Start-Process -Verb RunAs -Wait` is used because NSIS cannot wait for an
;     elevated child started with `ExecShell "runas"`, and the uninstaller must
;     not race ahead and delete files the service still holds open.
;   * The command line is passed as a single-quoted PowerShell argument. An
;     earlier version pointed at a helper batch file with
;     `-ArgumentList '/c','<path>'`; PowerShell joins the array elements with a
;     plain space, so any path containing a space (a user name with a space is
;     enough) silently broke the call. Passing cmd's command text instead means
;     no temporary file is written and no path has to survive the trip.
;   * `cmd /c` treats everything after /c as the command, so spaces inside the
;     command text are harmless.
;
; $R1 is preserved; $R0 is clobbered but saved and restored.
!macro CpumRunElevatedCmd
  Push $R0
  Push $R1
  DetailPrint "CPUM: running elevated: $R1"
  StrCpy $R0 "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command $\"Start-Process -FilePath cmd.exe -ArgumentList '/c','$R1' -Verb RunAs -Wait$\""
  nsExec::ExecToLog '$R0'
  Pop $R0
  StrCmp "$R0" "0" +2
  DetailPrint "CPUM: elevated helper exited with $R0 (5 = access denied, 1223 = UAC declined)"
  Pop $R1
  Pop $R0
!macroend

; Create / delete the "the user wants the service installed" marker.
; $9 is saved and restored, so callers may use any other register.
!macro CpumServiceFlagWrite
  Push $9
  CreateDirectory "$APPDATA\com.open-nexa.cpum"
  ClearErrors
  FileOpen $9 "${CPUM_SERVICE_FLAG}" w
  ${If} $9 != ""
    FileWrite $9 "installed$\r$\n"
    FileClose $9
  ${EndIf}
  Pop $9
!macroend

!macro CpumServiceFlagClear
  Delete "${CPUM_SERVICE_FLAG}"
!macroend

; ---------------------------------------------------------------------------
; Before install / upgrade: the service keeps cpum_service.exe open, which
; would block the file copy. Try an un-elevated stop first (silent, and the
; common case is "no service installed at all"), then escalate.
; ---------------------------------------------------------------------------
!macro NSIS_HOOK_PREINSTALL
  Push $R0
  Push $R1
  Push $R2

  DetailPrint "CPUM: stopping ${CPUM_SERVICE_NAME} before install"
  nsExec::ExecToLog '"$SYSDIR\sc.exe" stop "${CPUM_SERVICE_NAME}"'
  Pop $R2
  ; 1060 = not installed, 1062 = already stopped, 1072 = already marked for
  ; deletion: nothing to wait for. Anything else means the service exists and
  ; we need to make sure it actually let go of the binary.
  StrCmp "$R2" "1060" cpum_pre_done
  StrCmp "$R2" "1062" cpum_pre_done
  StrCmp "$R2" "1072" cpum_pre_done
  StrCmp "$R2" "0" 0 cpum_pre_elevate

  ; The stop was accepted; if the binary is already writable we are done.
  IfFileExists "$INSTDIR\cpum_service.exe" 0 cpum_pre_done
  ClearErrors
  FileOpen $R0 "$INSTDIR\cpum_service.exe" a
  IfErrors 0 cpum_pre_unlocked
  FileClose $R0

  ; Still locked, or the stop was refused (typically access denied): escalate.
  cpum_pre_elevate:
  StrCpy $R1 'sc.exe stop "${CPUM_SERVICE_NAME}" & ping -n 4 127.0.0.1 >nul'
  !insertmacro CpumRunElevatedCmd

  ; Wait until the old binary is writable again; a service that is still
  ; shutting down keeps it locked and would break the extraction below.
  IfFileExists "$INSTDIR\cpum_service.exe" 0 cpum_pre_done
  StrCpy $R2 0
  cpum_pre_wait:
    ClearErrors
    FileOpen $R0 "$INSTDIR\cpum_service.exe" a
    IfErrors 0 cpum_pre_unlocked
    FileClose $R0
    IntOp $R2 $R2 + 1
    IntCmp $R2 20 cpum_pre_done
    Sleep 500
    Goto cpum_pre_wait

  cpum_pre_unlocked:
  FileClose $R0

  cpum_pre_done:
  Pop $R2
  Pop $R1
  Pop $R0
!macroend

; ---------------------------------------------------------------------------
; After install / upgrade: migrate rule files into the per-user data directory,
; then rebind or restore the service.
; ---------------------------------------------------------------------------
!macro NSIS_HOOK_POSTINSTALL
  Push $R0
  Push $R1
  Push $R2
  Push $R3

  ; ---- Migrate rule files into the per-user data directory ----------------
  ; $COMMONAPPDATA / $PROGRAMDATA are not valid NSIS constants (they stay
  ; literal and silently create a junk folder inside $INSTDIR), so read the
  ; machine-level ProgramData directory from the environment.
  StrCpy $R0 "$APPDATA\com.open-nexa.cpum"
  CreateDirectory "$R0"

  ReadEnvStr $R2 "ProgramData"
  StrCmp "$R2" "" 0 +2
    StrCpy $R2 "C:\ProgramData"
  StrCpy $R2 "$R2\cpum"

  IfFileExists "$R0\affinity_rules.json" cpum_rules_migrated
  IfFileExists "$R2\affinity_rules.json" 0 cpum_try_previous_id_rules
    CopyFiles /SILENT "$R2\affinity_rules.json" "$R0\affinity_rules.json"
    Goto cpum_rules_migrated
  cpum_try_previous_id_rules:
  ; Builds released before the bundle identifier changed kept their rules in
  ; %APPDATA%\com.eason.cpum. Check it before the older locations below.
  IfFileExists "${CPUM_DATA_DIR_PREVIOUS_ID}\affinity_rules.json" 0 cpum_try_legacy_rules
    CopyFiles /SILENT "${CPUM_DATA_DIR_PREVIOUS_ID}\affinity_rules.json" "$R0\affinity_rules.json"
    Goto cpum_rules_migrated
  cpum_try_legacy_rules:
  IfFileExists "$APPDATA\cpum\affinity_rules.json" 0 cpum_rules_migrated
    CopyFiles /SILENT "$APPDATA\cpum\affinity_rules.json" "$R0\affinity_rules.json"
  cpum_rules_migrated:

  IfFileExists "$R0\probalance.json" cpum_pb_migrated
  IfFileExists "$R2\probalance.json" 0 cpum_try_previous_id_pb
    CopyFiles /SILENT "$R2\probalance.json" "$R0\probalance.json"
    Goto cpum_pb_migrated
  cpum_try_previous_id_pb:
  IfFileExists "${CPUM_DATA_DIR_PREVIOUS_ID}\probalance.json" 0 cpum_pb_migrated
    CopyFiles /SILENT "${CPUM_DATA_DIR_PREVIOUS_ID}\probalance.json" "$R0\probalance.json"
  cpum_pb_migrated:

  ; ---- Service -----------------------------------------------------------
  ; Installing or updating the app never *creates* the service on its own:
  ; that is an explicit, elevated action performed from inside the app. What
  ; this hook does is (a) rebind an already installed service to the new
  ; install / rules directory and (b) put the service back when an upgrade
  ; removed it through the previous version's uninstaller - which is only
  ; done when the user's marker file says the service belongs there.

  ; Tauri maps bundle resources next to the installed executable, but older
  ; builds shipped them in a resources subdirectory. Detect the actual
  ; location so upgrades keep working across layout changes. Without the
  ; binary there is nothing to register, so bail out early.
  StrCpy $R3 "$INSTDIR\cpum_service.exe"
  IfFileExists "$R3" 0 cpum_svc_in_resources
    Goto cpum_svc_path_ready
  cpum_svc_in_resources:
    StrCpy $R3 "$INSTDIR\resources\cpum_service.exe"
  cpum_svc_path_ready:
  IfFileExists "$R3" 0 cpum_post_done

  nsExec::ExecToStack '"$SYSDIR\sc.exe" query "${CPUM_SERVICE_NAME}"'
  Pop $R2
  Pop $R1
  StrCmp "$R2" "0" cpum_post_svc_exists

  ; Not registered. Restore it only if the user had it before this install;
  ; PREUNINSTALL leaves the marker behind when it runs as part of an upgrade.
  ; The previous identifier's marker counts too: the uninstaller that wrote it
  ; belongs to a build that used the old data directory.
  IfFileExists "${CPUM_SERVICE_FLAG}" 0 cpum_post_check_previous_flag
  Goto cpum_post_svc_restore
  cpum_post_check_previous_flag:
  IfFileExists "${CPUM_SERVICE_FLAG_PREVIOUS_ID}" 0 cpum_post_done
  cpum_post_svc_restore:
  DetailPrint "CPUM: restoring ${CPUM_SERVICE_NAME} (marker found, service missing)"
  StrCpy $R2 'sc.exe create "${CPUM_SERVICE_NAME}" binPath= "\"$R3\" \"$R0\"" start= auto DisplayName= "CPU Affinity Manager Service" & sc.exe description "${CPUM_SERVICE_NAME}" "Automatically applies CPU affinity rules to running processes on boot and process launch." & sc.exe start "${CPUM_SERVICE_NAME}"'
  Goto cpum_post_svc_run

  cpum_post_svc_exists:
  ; Registered: remember it, so the next upgrade can restore it if the
  ; previous version's uninstaller takes it away, then rebind it.
  !insertmacro CpumServiceFlagWrite
  StrCpy $R2 'sc.exe config "${CPUM_SERVICE_NAME}" binPath= "\"$R3\" \"$R0\"" start= auto & sc.exe start "${CPUM_SERVICE_NAME}"'

  cpum_post_svc_run:
  StrCpy $R1 $R2
  !insertmacro CpumRunElevatedCmd

  cpum_post_done:
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
!macroend

; ---------------------------------------------------------------------------
; Before uninstall: stop and delete the optional service, otherwise it keeps
; pointing at a deleted cpum_service.exe and lingers forever.
;
; Two situations reach this hook:
;   * the user really is uninstalling (Apps & Features, uninstall.exe): the
;     service goes away and so does the marker, so a later reinstall starts
;     clean instead of resurrecting a service nobody asked for;
;   * the installer runs it as the "uninstall before installing" half of an
;     upgrade (it passes `_?=<dir>`, which a direct run never has): the
;     service still has to be removed so the new files can be written, but the
;     marker is written first so the upgraded version puts it back.
; The updater path is different again - it runs this installer with /UPDATE
; and never touches the old uninstaller, so $UpdateMode is checked first.
; ---------------------------------------------------------------------------
!macro NSIS_HOOK_PREUNINSTALL
  Push $R0
  Push $R1
  Push $R2
  Push $R3

  ; The updater only wants the files replaced; the service must survive it.
  StrCmp "$UpdateMode" "1" cpum_preun_done

  ; $R3 = 1 when this uninstall is a step of an upgrade, 0 for a real one.
  ${GetOptions} $CMDLINE "_?=" $R3
  IfErrors 0 cpum_preun_upgrade
    StrCpy $R3 "0"
    Goto cpum_preun_context_known
  cpum_preun_upgrade:
    StrCpy $R3 "1"
  cpum_preun_context_known:

  DetailPrint "CPUM: checking for ${CPUM_SERVICE_NAME}"
  nsExec::ExecToStack '"$SYSDIR\sc.exe" query "${CPUM_SERVICE_NAME}"'
  Pop $R2
  Pop $R1

  StrCmp "$R3" "0" 0 cpum_preun_maybe_keep
    ; Real uninstall: the user is done with the app.
    !insertmacro CpumServiceFlagClear
    Goto cpum_preun_stop

  cpum_preun_maybe_keep:
  StrCmp "$R2" "0" 0 cpum_preun_done
    ; Upgrade with the service installed: remember it before the previous
    ; version's copy of this hook is asked to remove it.
    !insertmacro CpumServiceFlagWrite

  cpum_preun_stop:
  StrCmp "$R2" "0" 0 cpum_preun_done

  ; Stop without elevation first: it succeeds on setups where the user owns the
  ; service and keeps that path completely UAC-free.
  nsExec::ExecToLog '"$SYSDIR\sc.exe" stop "${CPUM_SERVICE_NAME}"'
  Pop $R2

  ; One elevated pass for stop + kill + delete. `taskkill` is the safety net
  ; for a service that ignores the stop control: without it `sc delete` only
  ; marks the service and the process keeps holding the binary.
  StrCpy $R1 'sc.exe stop "${CPUM_SERVICE_NAME}" & ping -n 4 127.0.0.1 >nul & taskkill /F /IM cpum_service.exe >nul 2>&1 & sc.exe delete "${CPUM_SERVICE_NAME}"'
  !insertmacro CpumRunElevatedCmd

  ; Wait until the binary can actually be removed, so the Delete in the
  ; uninstall section does not silently fail and leave a half-removed folder.
  StrCpy $R2 0
  cpum_preun_wait:
    IfFileExists "$INSTDIR\cpum_service.exe" 0 cpum_preun_done
    Delete "$INSTDIR\cpum_service.exe"
    IfFileExists "$INSTDIR\cpum_service.exe" 0 cpum_preun_done
    IntOp $R2 $R2 + 1
    IntCmp $R2 20 cpum_preun_done
    Sleep 500
    Goto cpum_preun_wait

  cpum_preun_done:
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
!macroend
