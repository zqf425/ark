; ark ships arkd as an independent sidecar. It can outlive the desktop shell
; (the shell was force-closed, or the shell merely reused a runtime started
; elsewhere) and then keeps its own exe open, so NSIS `File` fails with
; "Error opening file for writing: ...\arkd.exe".
;
; Upgrades stay in place: the installer replaces the files of the previous
; installation instead of uninstalling it first, so a failed upgrade never
; leaves the user without a working copy. Every process that can hold those
; files is closed before the copy, and the current binaries are backed up so
; .onInstFailed can put them back.
!include LogicLib.nsh

!macro ark_stop_sidecar
  ${If} ${FileExists} "$INSTDIR\arkd.exe"
    ; Match the full executable path, not the image name: another installation
    ; or a user-managed arkd may legitimately be running at the same time.
    ClearErrors
    InitPluginsDir
    CreateDirectory "$PLUGINSDIR"
    ClearErrors
    FileOpen $0 "$PLUGINSDIR\ark-stop-sidecar.ps1" w
    ${If} ${Errors}
      DetailPrint "warning: could not create the sidecar shutdown script."
    ${Else}
      ClearErrors
      FileWrite $0 'param([Parameter(Mandatory = $$true)][string]$$ExecutablePath)$\r$\n'
      FileWrite $0 '$$ErrorActionPreference = "Stop"$\r$\n'
      FileWrite $0 '$$targets = @(Get-Process -Name arkd -ErrorAction SilentlyContinue | Where-Object { $$_.Path -eq $$ExecutablePath })$\r$\n'
      FileWrite $0 'if ($$targets.Count -eq 0) { exit 0 }$\r$\n'
      FileWrite $0 '$$targets | Stop-Process -Force$\r$\n'
      FileWrite $0 'exit 0$\r$\n'
      FileClose $0
      ${If} ${Errors}
        DetailPrint "warning: could not write the sidecar shutdown script."
      ${Else}
        nsExec::Exec '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\ark-stop-sidecar.ps1" "$INSTDIR\arkd.exe"'
        Pop $0
        ${If} $0 != 0
          DetailPrint "warning: failed to stop $INSTDIR\arkd.exe (exit code $0)."
        ${EndIf}
      ${EndIf}
    ${EndIf}
    ; Process termination returns before the kernel drops the image section.
    Sleep 800
  ${EndIf}
!macroend

!macro ark_backup_binaries
  CreateDirectory "$INSTDIR\.ark-upgrade-backup"
  CopyFiles /SILENT "$INSTDIR\${MAINBINARYNAME}.exe" "$INSTDIR\.ark-upgrade-backup"
  CopyFiles /SILENT "$INSTDIR\arkd.exe" "$INSTDIR\.ark-upgrade-backup"
  CopyFiles /SILENT "$INSTDIR\WebView2Loader.dll" "$INSTDIR\.ark-upgrade-backup"
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro ark_stop_sidecar
  !insertmacro ark_backup_binaries
!macroend

!macro NSIS_HOOK_POSTINSTALL
  RMDir /r "$INSTDIR\.ark-upgrade-backup"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro ark_stop_sidecar
  RMDir /r "$INSTDIR\.ark-upgrade-backup"
!macroend

; A failed or aborted install restores the binaries captured above, so the
; previous version keeps working instead of leaving a half-copied directory.
Function .onInstFailed
  ${If} ${FileExists} "$INSTDIR\.ark-upgrade-backup\*.*"
    CopyFiles /SILENT "$INSTDIR\.ark-upgrade-backup\*.*" "$INSTDIR"
    ${IfNot} ${Silent}
      MessageBox MB_ICONINFORMATION "ark setup did not finish. The previous version has been restored."
    ${EndIf}
  ${EndIf}
FunctionEnd
