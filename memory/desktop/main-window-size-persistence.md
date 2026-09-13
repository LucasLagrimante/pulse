# Persistência do tamanho da janela principal

- A janela `main` usa `tauri-plugin-window-state` para restaurar o tamanho escolhido pelo usuário e o estado maximizado na próxima execução.
- O estado de visibilidade não é persistido, pois iniciar oculto pertence ao fluxo independente `--minimized-to-tray`.
- A janela `tray-flyout` é excluída pelo filtro do plugin e continua com as dimensões fixas definidas em `tauri.conf.json`.
- `width`, `height`, `minWidth` e `minHeight` em `tauri.conf.json` continuam sendo os valores padrão e os limites usados quando ainda não existe estado salvo.
