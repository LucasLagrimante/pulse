---
name: assinatura-signpath-ci
description: Instalador do Pulse é publicado SEM assinatura — SignPath Foundation recusou (2026-11-05); etapa SignPath do CI fica desligada até haver certificado
module: release
metadata:
  type: project
  pin: true
---
**Why:** Em 2026-11-05 a SignPath Foundation recusou o pedido por falta de visibilidade pública (stars, forks, menções externas); convidou a reaplicar depois. O usuário optou por seguir sem assinatura em vez de pagar (Certum OSS, SignPath paga, Azure Artifact Signing exige CNPJ/EUA). Desde 2024 nem OV tira o SmartScreen de cara.
**How to apply:**
- Nunca anunciar "assinado"/"SignPath Foundation" em landing, README ou notas de Release enquanto `vars.SIGNPATH_ORGANIZATION_ID` não existir; o texto vigente é "ainda não tem assinatura" + conferir SHA-256.
- Release: `git tag v<versão> && git push --tags` → `release.yml` publica sem assinatura → `npm run lp:sync:release` (SHA do asset do CI) → push no `pulse-lp`. `release:publish -- --unsigned` é só fallback local.
- Reaplicar à Foundation quando houver tração; a etapa SignPath do `release.yml` liga só com as variables, sem outra mudança.
- `npm ci` no CI exige lock em sincronia: `@tauri-apps/*` JS segue major.minor do crate, não o patch.
