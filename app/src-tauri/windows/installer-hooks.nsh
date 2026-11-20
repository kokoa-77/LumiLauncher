!define LUMI_SETUP_DIR "${__FILEDIR__}"
!define LUMI_ONBOARDING_MARKER ".lumi-onboarding-pending"
!define LUMI_GAME_LOCATION ".lumi-setup-game-dir"
!define LUMI_PACK_CLASS "Software\Classes\${BUNDLEID}.mrpack"
!define LUMI_PACK_EXT "Software\Classes\.mrpack"
!tempfile LUMI_COMPILED_UI
!system 'powershell.exe -NoLogo -NoProfile -NonInteractive -File "${LUMI_SETUP_DIR}\frontend\Build-SetupFrontend.ps1" -OutputPath "${LUMI_COMPILED_UI}"' = 0

Var LumiFreshInstall
Var LumiWorker
Var LumiStatusFile
Var LumiGamePath
Var LumiGameSelection
Var LumiDesktop
Var LumiPack
Var LumiFoundCount

!macro LumiFrontend prefix action engine
Function ${prefix}LumiFrontend
  ${GetOptions} $CMDLINE "/LUMI_WORKER" $0
  ${IfNot} ${Errors}
    ${IfNot} ${Silent}
      SetErrorLevel 2
      Abort
    ${EndIf}
    !if "${action}" == "remove"
      ${If} $UpdateMode = 1
        SetErrorLevel 2
        Abort
      ${EndIf}
    !endif
    Call ${prefix}LumiReadRequest
    Return
  ${EndIf}
  ${If} ${Silent}
    Return
  ${EndIf}
  StrCpy $2 "${action}"
  !if "${action}" == "install"
    ${If} $PassiveMode = 1
    ${AndIf} $UpdateMode = 1
      StrCpy $2 "update"
    ${EndIf}
  !endif
  ${If} $2 != "update"
    ${If} $PassiveMode = 1
    ${OrIf} $UpdateMode = 1
      Return
    ${EndIf}
  ${EndIf}

  InitPluginsDir
  File /oname=$PLUGINSDIR\LumiSetupUI.exe "${LUMI_COMPILED_UI}"
  ExecWait '"$PLUGINSDIR\LumiSetupUI.exe" $2 "${engine}" "$INSTDIR" "$APPDATA\${BUNDLEID}" "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"' $0
  ${If} ${Errors}
    StrCpy $0 2
  ${EndIf}
  !if "${action}" == "install"
    ${If} $0 = 0
    ${AndIf} $UpdateMode = 1
      Push $0
      Call LumiRelaunch
      Pop $0
    ${EndIf}
  !endif
  SetErrorLevel $0
  Quit
FunctionEnd

Function ${prefix}LumiReadRequest
  ReadEnvStr $0 "LUMI_SETUP_REQUEST"
  ReadEnvStr $LumiStatusFile "LUMI_SETUP_STATUS"
  ClearErrors
  FileOpen $1 "$0" r
  ${If} ${Errors}
    SetErrorLevel 2
    Abort
  ${EndIf}
  FileReadUTF16LE $1 $0
  StrCpy $0 $0 -2
  StrCpy $2 "${action}"
  !if "${action}" == "install"
    ${If} $UpdateMode = 1
      StrCpy $2 "update"
    ${EndIf}
  !endif
  ${If} $0 != $2
    FileClose $1
    SetErrorLevel 2
    Abort
  ${EndIf}
  FileReadUTF16LE $1 $LumiGamePath
  FileReadUTF16LE $1 $LumiGameSelection
  FileReadUTF16LE $1 $LumiDesktop
  FileReadUTF16LE $1 $LumiPack
  FileReadUTF16LE $1 $0
  FileClose $1
  StrCpy $LumiGamePath $LumiGamePath -2
  StrCpy $LumiGameSelection $LumiGameSelection -2
  StrCpy $LumiDesktop $LumiDesktop -2
  StrCpy $LumiPack $LumiPack -2
  StrCpy $DeleteAppDataCheckboxState $0 -2
  StrCpy $LumiWorker 1
FunctionEnd
!macroend

!macro LumiStatus value
  ${If} $LumiWorker = 1
  ${AndIf} $LumiStatusFile != ""
    ClearErrors
    FileOpen $0 "$LumiStatusFile" w
    ${IfNot} ${Errors}
      FileWrite $0 "${value}"
      FileClose $0
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; A new installation (including a reinstall after uninstalling) needs onboarding.
  ${If} $UpdateMode <> 1
  ${AndIfNot} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $LumiFreshInstall 1
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $LumiFreshInstall = 1
    ClearErrors
    FileOpen $0 "$INSTDIR\${LUMI_ONBOARDING_MARKER}" w
    ${If} ${Errors}
      DetailPrint "Lumi: .lumi-onboarding-pending could not be created."
    ${Else}
      FileClose $0
    ${EndIf}
  ${EndIf}
  ${If} $UpdateMode = 1
    !insertmacro LumiStatus "done"
  ${EndIf}
  ; Apply directory and shortcut selections only during manual installation.
  ${If} $LumiWorker = 1
  ${AndIf} $UpdateMode <> 1
    ClearErrors
    FileOpen $0 "$INSTDIR\${LUMI_GAME_LOCATION}" w
    ${If} ${Errors}
      SetErrorLevel 2
      Abort
    ${EndIf}
    FileWriteUTF16LE /BOM $0 "$LumiGameSelection$\r$\n$LumiGamePath"
    FileClose $0
    ${If} $LumiDesktop = 1
      Call CreateOrUpdateDesktopShortcut
    ${EndIf}
    ${If} $LumiPack = 1
      WriteRegStr HKCU "${LUMI_PACK_CLASS}" "" "Modrinth Modpack"
      WriteRegStr HKCU "${LUMI_PACK_CLASS}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
      WriteRegStr HKCU "${LUMI_PACK_CLASS}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --open-mrpack $\"%1$\""
      WriteRegStr HKCU "${LUMI_PACK_EXT}\OpenWithProgids" "${BUNDLEID}.mrpack" ""
      ReadRegStr $0 HKCR ".mrpack" ""
      ${If} $0 == ""
        WriteRegStr HKCU "${LUMI_PACK_EXT}" "" "${BUNDLEID}.mrpack"
      ${EndIf}
      System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
    ${EndIf}
    !insertmacro LumiStatus "scan"
    nsExec::ExecToStack /TIMEOUT=8000 '"$INSTDIR\${MAINBINARYNAME}.exe" --lumi-setup-scan "$LumiGamePath"'
    Pop $0
    Pop $LumiFoundCount
    ${If} $0 != 0
      StrCpy $LumiFoundCount ""
    ${EndIf}
    !insertmacro LumiStatus "done:$LumiFoundCount"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    Delete "$INSTDIR\${LUMI_ONBOARDING_MARKER}"
    Delete "$INSTDIR\${LUMI_GAME_LOCATION}"
    RMDir "$INSTDIR"
    ReadRegStr $0 HKCU "${LUMI_PACK_CLASS}\shell\open\command" ""
    ${If} $0 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --open-mrpack $\"%1$\""
      DeleteRegKey HKCU "${LUMI_PACK_CLASS}"
      DeleteRegValue HKCU "${LUMI_PACK_EXT}\OpenWithProgids" "${BUNDLEID}.mrpack"
      DeleteRegKey /ifempty HKCU "${LUMI_PACK_EXT}\OpenWithProgids"
      ReadRegStr $0 HKCU "${LUMI_PACK_EXT}" ""
      ${If} $0 == "${BUNDLEID}.mrpack"
        DeleteRegValue HKCU "${LUMI_PACK_EXT}" ""
      ${EndIf}
      DeleteRegKey /ifempty HKCU "${LUMI_PACK_EXT}"
      System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
    ${EndIf}
    !insertmacro LumiStatus "done"
  ${EndIf}
!macroend
