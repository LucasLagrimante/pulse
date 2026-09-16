---
name: vite-css-reload-wsl-mnt-c
description: Vite dev server em /mnt/c não invalida CSS alterado com frequência; reiniciar antes de validar visualmente
module: tooling
metadata:
  type: project
---
**Why:** Após editar `src/*.css` no WSL, `curl /src/<arquivo>.css` continuou servindo o conteúdo antigo e os screenshots refletiam regras obsoletas, gerando diagnósticos errados.

**How to apply:**
- Antes de validar estilos, confirmar o conteúdo servido com `curl "http://localhost:<porta>/src/identity.css?t=$(date +%s)"` procurando a regra nova.
- Se estiver desatualizado, matar o Vite (`pkill -9 -f "vite[.]js --port <porta>"`) e subir de novo; HMR não basta.
- No Chrome, `Network.clearBrowserCache` + `Network.setCacheDisabled` + `Page.reload({ignoreCache:true})` evitam CSS antigo vindo do próprio cache do navegador.
- Mesmo com o curl mostrando a regra nova, o `<style>` injetado pode continuar velho: confirmar via CDP em `document.styleSheets` (procurar o seletor e o `conditionText` do media query) antes de concluir que a regra "não funcionou".
- Subir/derrubar o dev server desanexado: `(setsid nohup npx vite --port 5199 --strictPort >/tmp/opencode/vite.log 2>&1 </dev/null &)`.
