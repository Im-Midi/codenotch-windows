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
      MessageBox MB_OK|MB_ICONSTOP "Could not safely remove Codenotch hooks. Check your Claude settings and any .codenotch-bak backup, then retry uninstalling." /SD IDOK
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
!macroend
