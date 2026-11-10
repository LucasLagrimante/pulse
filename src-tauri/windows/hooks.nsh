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
!macroend
