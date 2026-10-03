; deoxidizer Windows NSIS Installer Script
; Builds deoxidizer-setup.exe

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"
!include "WordFunc.nsh"

; Standard NSIS builds read registry strings into a 1024-character buffer.
; A longer user PATH makes ReadRegStr fail (error flag set, empty result),
; and writing that back would destroy the PATH. The installer and
; uninstaller therefore only edit PATH when the read succeeded and the
; result leaves room to append, and otherwise tell the user to edit PATH by
; hand. A large-strings NSIS build (NSIS_MAX_STRLEN=8192) handles more
; users automatically.
!if ${NSIS_MAX_STRLEN} < 8192
  !warning "NSIS_MAX_STRLEN is ${NSIS_MAX_STRLEN}; users with long PATHs must add deoxidizer to PATH manually."
!endif

Unicode True

Name "deoxidizer"
!ifndef VERSION
!define VERSION "0.0.0"
!endif
!ifndef FILE_VERSION
!define FILE_VERSION "0.0.0.0"
!endif
VIProductVersion "${FILE_VERSION}"
VIAddVersionKey "ProductName" "deoxidizer"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "Publisher" "BurntToasters"
VIAddVersionKey "FileDescription" "deoxidizer installer"
VIAddVersionKey "LegalCopyright" "GPL-3.0-or-later"
!ifndef OUTPUT_DIR
!define OUTPUT_DIR "release"
!endif
!ifndef BUILD_DIR
!define BUILD_DIR "target\release"
!endif
!ifndef OUTPUT_NAME
!define OUTPUT_NAME "deoxidizer-setup.exe"
!endif
OutFile "${OUTPUT_DIR}\${OUTPUT_NAME}"
InstallDir "$LOCALAPPDATA\Programs\deoxidizer"
InstallDirRegKey HKCU "Software\deoxidizer" "InstallDir"
RequestExecutionLevel user

!define MUI_ICON "${NSISDIR}\Contrib\Graphics\Icons\modern-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\modern-uninstall.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath $INSTDIR
  File "${BUILD_DIR}\deoxidizer.exe"
  File "${BUILD_DIR}\deox.exe"
  File /oname=LICENSE.txt "LICENSE"

  ; Write uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Registry keys
  WriteRegStr HKCU "Software\deoxidizer" "InstallDir" $INSTDIR
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "DisplayName" "deoxidizer"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "InstallLocation" $INSTDIR
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "Publisher" "BurntToasters"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "NoRepair" 1

  ; Add to user PATH (complete semicolon-delimited entries only).
  ClearErrors
  ReadRegStr $0 HKCU "Environment" "Path"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONINFORMATION "Could not read your user PATH safely (it may be longer than this installer supports). Add $INSTDIR to PATH manually."
  ${Else}
    StrLen $3 "$0;$INSTDIR;"
    ${If} $3 >= ${NSIS_MAX_STRLEN}
      MessageBox MB_OK|MB_ICONINFORMATION "Your user PATH is too long for this installer to edit safely. Add $INSTDIR to PATH manually."
    ${Else}
      StrCpy $1 ";$0;"
      ${WordReplace} $1 ";$INSTDIR;" ";" "+" $2
      ${If} $1 == $2
        ${If} $0 == ""
          WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR"
        ${Else}
          WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR;$0"
        ${EndIf}
        SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
      ${EndIf}
    ${EndIf}
  ${EndIf}
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\deoxidizer.exe"
  Delete "$INSTDIR\deox.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  ; Remove from PATH only when the full value was read; never write back a
  ; failed or truncated read.
  ClearErrors
  ReadRegStr $0 HKCU "Environment" "Path"
  ${IfNot} ${Errors}
  ${AndIf} $0 != ""
    StrLen $3 "$0"
    IntOp $4 ${NSIS_MAX_STRLEN} - 1
    ${If} $3 < $4
      StrCpy $1 ";$0;"
      ${WordReplace} $1 ";$INSTDIR;" ";" "+" $2
      ${If} $1 != $2
        ; Strip the delimiters added above.
        StrCpy $2 $2 "" 1
        StrLen $3 $2
        IntOp $3 $3 - 1
        ${If} $3 > 0
          StrCpy $2 $2 $3
        ${Else}
          StrCpy $2 ""
        ${EndIf}
        WriteRegExpandStr HKCU "Environment" "Path" $2
        SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
      ${EndIf}
    ${EndIf}
  ${EndIf}

  ; Remove registry keys
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer"
  DeleteRegKey HKCU "Software\deoxidizer"
SectionEnd
