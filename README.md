# Pulse

Aplicativo Windows-first para explicar atividade de arquivos, desempenho, espaço em disco, inicialização e sinais de segurança — sem conta e com dados locais por padrão.

## Estado atual

A base Tauri 2 + React está pronta. O módulo visual inicial já organiza as áreas do produto; os coletores nativos serão adicionados em etapas, com testes antes de cada comportamento novo.

A implementação Python original foi preservada em [`legacy-python/`](legacy-python/) e no commit `50ac7f4`.

## Requisitos

- Windows 11
- Node.js 22+
- Rust stable com target MSVC
- Microsoft C++ Build Tools (necessário para compilar Tauri)

## Desenvolvimento

```bash
npm install
npm run tauri dev
```

## Qualidade

```bash
npm test
npm run build
npm run tauri build -- --bundles nsis
```

O instalador NSIS é gerado em `src-tauri/target/release/bundle/nsis/`.

## Princípios

- Dados armazenados localmente.
- Métricas sem suporte explícito aparecem como indisponíveis; nunca como zero fictício.
- Sinais de risco trazem as evidências e não fazem alegação de malware.
- Ações potencialmente destrutivas não entram no beta.
