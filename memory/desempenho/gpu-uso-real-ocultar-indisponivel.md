---
name: gpu-uso-real-ocultar-indisponivel
description: Uso de GPU real via PDH GPU Engine no card Desempenho e ocultação total do gráfico quando o driver não expõe a métrica
module: desempenho
metadata:
  type: feedback
---
**Why:** O card GPU exibia barra vazia "Detectada" + etiqueta "Utilização não exposta por esta fonte", parecendo bug. O usuário pediu para medir de verdade via contadores e, se não houver o que mostrar, não mostrar nada.
**How to apply:** Meça com `\GPU Engine(*)\Utilization Percentage` no poll de overview (`process_usage::gpu_usage` → `gpu_percent` no `SystemOverview`), agregando por engine física: por processo vale a engine mais ocupada; o total é a engine mais ocupada somando os processos que a compartilham (semântica do Gerenciador de Tarefas). No card, renderize `DetailedResourceChart` apenas quando `gpuPercent` existir; sem fonte, omita gráfico/barra/etiqueta por completo.
