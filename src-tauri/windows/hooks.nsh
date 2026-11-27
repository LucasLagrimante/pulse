; Hooks do instalador NSIS do Pulse.
; Motivo: o Pulse fica residente na bandeja. Numa atualização, o NSIS
; executa o desinstalador antigo, que não consegue apagar o Pulse.exe
; travado — e o setup novo aborta com erros em cascata. Estes hooks
; encerram o processo antes de instalar e antes de desinstalar.
; Referência: installerHooks em tauri.conf.json (docs Tauri v2 NSIS).
;
; nsExec::Exec (e não ExecWait): taskkill é app de console e ExecWait abre
; uma janela de terminal por chamada. nsExec roda oculto e empilha o código
; de saída, descartado aqui preservando $0 (o template do Tauri usa $0).
; taskkill retorna nonzero se não houver processo — sem abortar.

!macro PULSE_KILL_HIDDEN IMAGE
  Push $0
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${IMAGE} /T'
  Pop $0 ; código de saída do taskkill (ignorado)
  Pop $0 ; restaura $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Encerra instância antiga antes de copiar os arquivos novos.
  !insertmacro PULSE_KILL_HIDDEN "pulse-windows.exe"
  !insertmacro PULSE_KILL_HIDDEN "Pulse.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Garante que o exe não esteja travado ao remover os arquivos.
  !insertmacro PULSE_KILL_HIDDEN "pulse-windows.exe"
  !insertmacro PULSE_KILL_HIDDEN "Pulse.exe"
  ; Desinstalação de verdade (não atualização): remove o modo administrador da
  ; tecla Windows — tarefa agendada PulseWinKeyHook e a cópia protegida em
  ; Arquivos de Programas. Fica fora da pasta do Pulse e exige administrador
  ; (um aviso do UAC, só se o modo estiver instalado).
  ${If} $UpdateMode <> 1
  ${AndIf} ${FileExists} "$PROGRAMFILES64\Pulse WinKey\pulse-windows.exe"
    ExecShellWait "runas" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" "-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command $\"Get-ScheduledTask -TaskName 'PulseWinKeyHook' -ErrorAction SilentlyContinue | Stop-ScheduledTask -ErrorAction SilentlyContinue; Get-Process -Name 'pulse-windows' -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '*\Pulse WinKey\*' } | Stop-Process -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 300; Unregister-ScheduledTask -TaskName 'PulseWinKeyHook' -Confirm:$$false -ErrorAction SilentlyContinue; Remove-Item -LiteralPath '$PROGRAMFILES64\Pulse WinKey' -Recurse -Force -ErrorAction SilentlyContinue$\"" SW_HIDE
  ${EndIf}
!macroend
