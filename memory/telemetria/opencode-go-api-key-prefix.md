---
name: opencode-go-api-key-prefix
description: Aceitar chaves do OpenCode Go com prefixos modernos oc_sk_/oc- e legado sk-
module: telemetria
metadata:
  type: project
---
**Why:** O OpenCode Go atualizou o formato de chaves da plataforma para prefixos baseados em `oc` (ex: `oc_sk_`, `oc-sk-`, `oc-`), causando rejeição no salvamento caso o validador restrinja apenas ao prefixo legado `sk-`.
**How to apply:** Ao validar chaves do OpenCode Go no backend (`is_valid_opencode_go_key`) e na UI, permitir tanto prefixos modernos `oc` (case-insensitive, com ou sem separador) quanto o prefixo legado `sk-` com tamanho mínimo de 16 caracteres.
