; Hooks do instalador NSIS do Pulse.
; Motivo: o Pulse fica residente na bandeja. Numa atualização, o NSIS
; executa o desinstalador antigo, que não consegue apagar o Pulse.exe
; travado — e o setup novo aborta com erros em cascata. Estes hooks
; encerram o processo antes de instalar e antes de desinstalar.
; Referência: installerHooks em tauri.conf.json (docs Tauri v2 NSIS).

!macro NSIS_HOOK_PREINSTALL
  ; Encerra instância antiga antes de copiar os arquivos novos.
  ; taskkill retorna nonzero se não houver processo — sem abortar.
  ExecWait '"$SYSDIR\taskkill.exe" /F /IM pulse-windows.exe /T'
  ExecWait '"$SYSDIR\taskkill.exe" /F /IM Pulse.exe /T'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Garante que o exe não esteja travado ao remover os arquivos.
  ExecWait '"$SYSDIR\taskkill.exe" /F /IM pulse-windows.exe /T'
  ExecWait '"$SYSDIR\taskkill.exe" /F /IM Pulse.exe /T'
!macroend
