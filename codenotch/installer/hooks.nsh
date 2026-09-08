; Tauri's /UPDATE mode preserves integrations while replacing binaries.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
    ClearErrors
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" uninstall-installed-hooks' $0
    ${If} ${Errors}
      StrCpy $0 1
    ${EndIf}
    ${If} $0 <> 0
      MessageBox MB_OK|MB_ICONSTOP "Could not remove Codenotch hooks. Your Claude settings were not replaced. Check install.log in the Codenotch data folder, then retry uninstalling." /SD IDOK
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
!macroend
