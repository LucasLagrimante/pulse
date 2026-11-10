---
name: ia-revisao-servicos-envia-nao-windows
description: A revisão de segurança dos serviços com IA envia todos os serviços ativos fora de System32/SysWOW64, não só a categoria Revisão (que é ~0)
module: seguranca
metadata:
  type: project
---
**Why:** A categoria "Revisão" é restrita de propósito (0 de 313 na máquina real); mandar só ela à IA geraria um relatório vazio. O usuário quer a IA caçando keylogger/invasor, sem o Pulse resolver nada.
**How to apply:** `ai_services_advisor.rs` (reaproveita os helpers `pub(crate)` do `ai_disk_advisor.rs`) detalha os serviços em execução com executável fora das pastas do Windows ou categoria ≠ sistema, e lista os demais só pelo nome. O pré-prompt proíbe parar, desativar ou alterar qualquer coisa e rotular malware sem evidência; o parecer vai para `%TEMP%\pulse-services-security-review.md`, com caminho derivado no backend.
