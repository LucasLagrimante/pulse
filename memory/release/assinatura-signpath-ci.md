---
name: assinatura-signpath-ci
description: Instalador oficial é assinado pela SignPath Foundation no GitHub Actions; o build local não tem assinatura e tem outro SHA-256
module: release
metadata:
  type: project
---
**Why:** Em 2026-11-04 o usuário escolheu a SignPath Foundation (grátis para OSS) para tirar o aviso "Editor desconhecido" do SmartScreen. A SignPath só assina artefatos gerados em runner hospedado pelo GitHub a partir do código público, e a assinatura muda o hash do `.exe`.
**How to apply:**
- Release: `git tag v<versão> && git push --tags` → `.github/workflows/release.yml` (build, SignPath, `Get-AuthenticodeSignature`, upload na Release) → `npm run lp:sync:release` (SHA do asset assinado) → push no `pulse-lp`.
- `release:publish` exige `--unsigned`: nunca sobrescrever um instalador assinado com build local.
- Sem `vars.SIGNPATH_ORGANIZATION_ID` o CI publica **sem assinatura** e as notas da Release dizem isso (decisão do usuário: não esperar a aprovação). Nunca anunciar "assinado" na landing antes de haver certificado. Política de assinatura obrigatória fica no README.
- `npm ci` no CI exige lock em sincronia: pacotes JS `@tauri-apps/*` seguem o major.minor do crate, não o patch (api 2.11.5 não existe).
