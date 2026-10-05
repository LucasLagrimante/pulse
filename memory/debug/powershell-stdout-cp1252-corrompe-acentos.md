---
name: powershell-stdout-cp1252-corrompe-acentos
description: PowerShell grava stdout em CP1252 por padrão — forçar UTF-8 com $OutputEncoding/[Console]::OutputEncoding
module: debug
metadata:
  type: project
  pin: true
---

**Why:** `powershell.exe -Command ...` grava stdout na **página de código do console (CP1252 no Windows pt-BR)**, não em UTF-8. Resultado observado no Pulse: o serviço `UserDataSvc_12d840` aparecia na tela como **"Acesso a Dados de Usu�rio_12d840"** (acentos substituídos pelo caractere U+FFFD) — o `String::from_utf8_lossy` do Rust não tinha culpa; os bytes já estavam errados na origem.

**Como aplicar:** prefixar o comando com a redefinição de encoding:

```powershell
$ErrorActionPreference='Stop';
$OutputEncoding=[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);
Get-CimInstance Win32_Service | ...
```

- `$OutputEncoding` cobre o pipeline/redirecionamento; `[Console]::OutputEncoding` cobre a saída do host. **Definir os dois** é o que funciona de fato — um só não basta em todos os caminhos.
- `UTF8Encoding::new($false)` = sem BOM. O BOM (`new($true)`) polui a primeira linha do TSV e quebra o parser.
- Combine sempre com `-NoProfile -NonInteractive` e `crate::process::hidden_command` (sem janela).
- **Regressão testável:** o teste precisa falhar por conteúdo, não só por encoding — procure `'\u{FFFD}'` em todos os campos E exija que um valor acento-conhecido apareça literal (ex.: `display_name.contains("Usuário")`). Sem a segunda asserção, um snapshot vazio passaria o teste de integridade.
- Mesmo cuidado vale para `ConvertTo-Json`: se um dia precisar voltar a ele, decodifique UTF-8 explicitamente antes do `serde_json`.
- Armadilha relacionada já vista no projeto: parser de tabela do winget por **byte** vs **caractere** (`memory/seguranca/winget-tabela-localizada-byte-vs-char.md`). Texto acentuado quebra offset de duas formas — codificação e largura em bytes.