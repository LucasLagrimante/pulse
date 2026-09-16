# Cotas multiprovedor de assinaturas de IA

- O Pulse consulta Claude Code, Codex/ChatGPT, Antigravity e OpenRouter em paralelo, sempre em uma única tarefa de fundo, e retorna uma assinatura por provedor em vez de duplicar Windows e WSL.
- A primeira resposta é imediata: usa o último registro local do Codex e inicia um único worker de atualização. A interface faz polling genérico de estado (`running`/`ready`) sem indicar qual fonte está sendo consultada.
- Codex tenta em ordem: `wham/usage` oficial (auth.json), registros locais e, por último, `account/rateLimits/read` do app-server com processo sem janela. Claude usa diretamente `https://console.anthropic.com/api/oauth/usage`; Antigravity usa `retrieveUserQuotaSummary` do Cloud Code com credencial segura.
- A interface nunca recebe tokens, cookies, e-mails, chaves ou conteúdo bruto de processos. Nenhum PTY é aberto; processos auxiliares Windows usam `CREATE_NO_WINDOW`.
- A leitura de arquivos ignora links, limita profundidade a 3, arquivos a 2.500, tempo a 1,5 s e chama `yield_now()` a cada 15 arquivos.
- Todos os provedores convertem percentuais defensivamente. Campo ausente nunca vira zero e uma CLI que não exponha cota aparece como `Indisponível` com motivo.
- Se o horário de reset já passou, a janela é marcada como indisponível até surgir uma nova medição; não se presume 100% restante. O parser aceita `rate_limits`/`rate_limit` e reset absoluto ou relativo (ver memória `codex-rate-limit-payloads`).
- A resposta fica em cache por 60 segundos; o botão de atualização força nova leitura sem bloquear a janela.
- A chave OpenRouter é validada e guardada somente no Credential Manager (`keyring`); o endpoint `/api/v1/credits` retorna apenas saldo total/uso/saldo restante, exibido na página e no flyout do tray.
