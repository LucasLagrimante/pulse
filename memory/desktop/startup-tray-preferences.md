# Inicialização no tray e preferência de fechamento

- A entrada `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Pulse` deve iniciar o executável com `--minimized-to-tray`; o backend interpreta o argumento no `setup` depois de registrar o ícone da bandeja e oculta a janela principal.
- A escolha de minimizar no diálogo de fechamento é persistida localmente em `%LOCALAPPDATA%\Pulse\tray-preferences.json`. Ao fechar de novo, inclusive pelo controle nativo da janela, o backend oculta a janela diretamente e não emite novo diálogo.
- O botão de bandeja explícito continua apenas minimizando; somente a escolha no diálogo grava a preferência.
