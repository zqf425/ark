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
    nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM arkd.exe'
    Pop $0
    ; taskkill returns before the kernel drops the image section.
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
