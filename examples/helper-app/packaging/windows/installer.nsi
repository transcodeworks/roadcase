; An installer for an application packaged by roadcase: for one person, so
; that it needs no administrator, and so that an update needs none.
;
; `roadcase release windows` runs makensis on this script with defines (NAME,
; VERSION, VERSION4, PUBLISHER, EXE_NAME, OUT, UNINSTALL_KEY, ICON...) and a
; generated include, ${ROADCASE_NSH}, whose macros install the staged files,
; write the registry entries of the URL schemes and document types, and read
; the flags an update starts the installer with. The list of every define is
; in the documentation of [windows.nsis] (crates/roadcase-bundle/src/config.rs).
;
; The application updates itself by starting the next version's installer:
;   <installer> /S /UPDATE [/RELAUNCH=<arguments>]
; /S is NSIS's silent mode. /UPDATE says the application is to be started
; again once the files are in place, with the arguments of /RELAUNCH=.
;
; This is a starting point: an application's own script may ask a running
; copy to leave, take over an earlier installer's folder, or add pages.
Unicode true
ManifestDPIAware true
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "${ROADCASE_NSH}"

Name "${NAME}"
OutFile "${OUT}"
!ifdef ICON
  Icon "${ICON}"
  UninstallIcon "${ICON}"
!endif
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\${NAME}"
SetCompressor /SOLID lzma
VIProductVersion "${VERSION4}"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
!ifdef COPYRIGHT
  VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
!endif
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

; The uninstaller is written inside the installer, so it is signed while the
; installer is made: roadcase passes the signing command when it signs.
!ifdef SIGN_UNINSTALLER
  !uninstfinalize '${SIGN_UNINSTALLER}'
!endif

Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Var Update
Var Relaunch

Function .onInit
  SetShellVarContext current
  ; Where an earlier version is installed.
  ReadRegStr $0 HKCU "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
  !insertmacro ROADCASE_READ_UPDATE
  StrCpy $Update $R0
  StrCpy $Relaunch $R1
FunctionEnd

Section "${NAME}"
  ; An update starts while the application is still leaving, and holds its
  ; executable until it has left.
  !insertmacro ROADCASE_WAIT_UNTIL_WRITABLE
  !insertmacro ROADCASE_INSTALL_FILES
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; A person who removed the shortcut does not get it back from an update.
  ${If} $Update != 1
  ${OrIf} ${FileExists} "$SMPROGRAMS\${NAME}.lnk"
    CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\${EXE_NAME}"
  ${EndIf}

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" '"$INSTDIR\${EXE_NAME}"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"

  !insertmacro ROADCASE_REGISTER
SectionEnd

; An update starts the application again, with the arguments of /RELAUNCH=.
Section "-Relaunch"
  ${If} $Update == 1
    Exec '"$INSTDIR\${EXE_NAME}" $Relaunch'
  ${EndIf}
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  ; What the application wrote for the person (documents, settings) stays.
  !insertmacro ROADCASE_UNINSTALL_FILES
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${NAME}.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  !insertmacro ROADCASE_UNREGISTER
SectionEnd
