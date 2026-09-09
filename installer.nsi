; deoxidizer Windows NSIS Installer Script
; Builds deoxidizer-setup.exe

!include "MUI2.nsh"
!include "FileFunc.nsh"

Name "deoxidizer"
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
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "InstallLocation" $INSTDIR
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "Publisher" "BurntToasters"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer" "NoRepair" 1

  ; Add to user PATH
  ReadRegStr $0 HKCU "Environment" "Path"
  StrCmp $0 "" 0 +2
    StrCpy $0 ""
  ${If} $0 !~ "*$INSTDIR*"
    WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR;$0"
    ; Broadcast WM_SETTINGCHANGE
    SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  ${EndIf}
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\deoxidizer.exe"
  Delete "$INSTDIR\deox.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  ; Remove from PATH
  ReadRegStr $0 HKCU "Environment" "Path"
  ; Add delimiters so only a complete semicolon-separated entry is removed.
  StrCpy $1 ";$0;"
  ${WordReplace} $1 ";$INSTDIR;" ";" "+" $1
  StrCpy $2 $1 "" 1
  StrLen $3 $2
  IntOp $3 $3 - 1
  ${If} $3 > 0
    StrCpy $2 $2 $3
  ${Else}
    StrCpy $2 ""
  ${EndIf}
  WriteRegExpandStr HKCU "Environment" "Path" $2
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000

  ; Remove registry keys
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\deoxidizer"
  DeleteRegKey HKCU "Software\deoxidizer"
SectionEnd
