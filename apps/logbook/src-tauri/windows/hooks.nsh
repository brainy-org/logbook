; The Windows installer's hooks (tauri.windows.conf.json). libparakeet,
; which the app loads to transcribe on the computer, needs the Visual C++
; runtime (msvcp140.dll, vcruntime140.dll, vcomp140.dll), which a fresh
; Windows lacks. The installer carries Microsoft's redistributable
; (vc_redist.x64.exe, fetched by `scripts/app.sh windows`) and runs it,
; quietly, only when the runtime is missing or older than 14.40 (Visual
; Studio 2022 17.10): an update finds it there and asks nothing. It needs
; administrator rights, so Windows asks for them that once; refused, the app
; installs all the same and says what is missing when it opens libparakeet.

!macro LOGBOOK_VC_RUNTIME_PRESENT
  ; $R0 = 1 when an x64 runtime 14.40 or later is installed, in either
  ; registry view.
  StrCpy $R0 0
  ReadRegDWORD $R1 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  ReadRegDWORD $R2 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Minor"
  ${If} $R1 == 1
  ${AndIf} $R2 >= 40
    StrCpy $R0 1
  ${EndIf}
  ${If} $R0 == 0
    SetRegView 64
    ReadRegDWORD $R1 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
    ReadRegDWORD $R2 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Minor"
    SetRegView lastused
    ${If} $R1 == 1
    ${AndIf} $R2 >= 40
      StrCpy $R0 1
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro LOGBOOK_VC_RUNTIME_PRESENT
  ${If} $R0 == 0
    DetailPrint "Installing the Microsoft Visual C++ runtime…"
    ; ShellExecute, not CreateProcess: the redistributable asks for
    ; administrator rights, which a per-user installer does not have.
    ExecShellWait "open" "$INSTDIR\vc_redist.x64.exe" "/install /quiet /norestart" SW_HIDE
  ${EndIf}
  ; 25 MB the app does not need once it has run.
  Delete "$INSTDIR\vc_redist.x64.exe"
!macroend
