; Voice Notepad installer.
;
; Per-user, into %LOCALAPPDATA%\Programs: no UAC prompt, no admin rights, and
; the app never ends up running elevated. Everything it writes afterwards --
; settings and the speech model -- lives under the same user's AppData, so
; install and runtime agree about where things are.
;
; Built with: makensis -DVERSION=... -DSOURCE_EXE=... -DOUTFILE=... installer.nsi

Unicode true

!ifndef VERSION
  !define VERSION "1.0.0"
!endif
!ifndef SOURCE_EXE
  !error "SOURCE_EXE must be defined (path to VoiceNotepad.exe)"
!endif
!ifndef OUTFILE
  !define OUTFILE "VoiceNotepad-Setup.exe"
!endif
!ifndef ASSETS
  !define ASSETS "..\assets"
!endif
!ifndef LICENSE_FILE
  !define LICENSE_FILE "..\LICENSE"
!endif

!define APPNAME     "Voice Notepad"
!define EXENAME     "VoiceNotepad.exe"
!define REGKEY      "Software\Microsoft\Windows\CurrentVersion\Uninstall\VoiceNotepad"

Name           "${APPNAME}"
OutFile        "${OUTFILE}"
InstallDir     "$LOCALAPPDATA\Programs\${APPNAME}"
InstallDirRegKey HKCU "Software\VoiceNotepad" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

VIProductVersion "${VERSION}.0"
VIAddVersionKey  "ProductName"     "${APPNAME}"
VIAddVersionKey  "FileDescription" "${APPNAME} installer"
VIAddVersionKey  "FileVersion"     "${VERSION}"
VIAddVersionKey  "ProductVersion"  "${VERSION}"
VIAddVersionKey  "LegalCopyright"  "MIT licensed"

!include "MUI2.nsh"
!include "LogicLib.nsh"

!define MUI_ICON   "${ASSETS}\voice-notepad.ico"
!define MUI_UNICON "${ASSETS}\voice-notepad.ico"
!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${LICENSE_FILE}"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES

!define MUI_FINISHPAGE_TITLE "Voice Notepad is installed"
!define MUI_FINISHPAGE_TEXT  "Open it from the Start menu.$\r$\n$\r$\nThe first time it runs it downloads the speech model it needs, and shows a progress bar while it does. After that it works entirely offline."
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Voice Notepad" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "/oname=${EXENAME}" "${SOURCE_EXE}"
  File "/oname=voice-notepad.ico" "${ASSETS}\voice-notepad.ico"
  File "/oname=LICENSE.txt" "${LICENSE_FILE}"

  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortCut  "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk" \
                  "$INSTDIR\${EXENAME}" "" "$INSTDIR\voice-notepad.ico" 0

  WriteRegStr HKCU "Software\VoiceNotepad" "InstallDir" "$INSTDIR"

  ; Appear in Settings > Apps, so it uninstalls like anything else.
  WriteRegStr   HKCU "${REGKEY}" "DisplayName"     "${APPNAME}"
  WriteRegStr   HKCU "${REGKEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr   HKCU "${REGKEY}" "DisplayIcon"     "$INSTDIR\voice-notepad.ico"
  WriteRegStr   HKCU "${REGKEY}" "Publisher"       "${APPNAME}"
  WriteRegStr   HKCU "${REGKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKCU "${REGKEY}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegDWORD HKCU "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${REGKEY}" "NoRepair" 1

  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortCut "$DESKTOP\${APPNAME}.lnk" \
                 "$INSTDIR\${EXENAME}" "" "$INSTDIR\voice-notepad.ico" 0
SectionEnd

LangString DESC_SecMain    ${LANG_ENGLISH} "The Voice Notepad program and its Start menu entry."
LangString DESC_SecDesktop ${LANG_ENGLISH} "Also put a shortcut on the desktop."

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain}    $(DESC_SecMain)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_SecDesktop)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\${EXENAME}"
  Delete "$INSTDIR\voice-notepad.ico"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir  "$INSTDIR"

  Delete "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk"
  RMDir  "$SMPROGRAMS\${APPNAME}"
  Delete "$DESKTOP\${APPNAME}.lnk"

  DeleteRegKey HKCU "${REGKEY}"
  DeleteRegKey HKCU "Software\VoiceNotepad"

  ; The speech model can be well over a gigabyte, and the settings are small.
  ; Ask rather than assume: a reinstall would otherwise download it all again.
  MessageBox MB_YESNO|MB_ICONQUESTION \
    "Also delete the downloaded speech model and your settings?$\r$\n$\r$\nChoose No if you plan to reinstall." \
    IDNO keep_data
    RMDir /r "$APPDATA\VoiceNotepad"
  keep_data:
SectionEnd
