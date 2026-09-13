# Trilha transparente de Debug e auditoria

- O módulo Debug permanece acessível mesmo quando a captura está pausada, para que o histórico da execução possa ser consultado.
- Cada evento instrumentado expõe sequência, horário, operação canônica, categoria, origem, duração, resultado, severidade e mensagem curta.
- A interface deve declarar os limites reais: a trilha é local, fica somente em memória, é descartada ao encerrar e retém no máximo 500 eventos.
- O relatório pode ser filtrado, copiado ou exportado como JSON; limpar afeta somente o histórico volátil da execução atual.
- Novas operações só devem aparecer como auditadas depois de realmente chamarem `debug::record`; a UI usa a expressão "operações instrumentadas" para não prometer cobertura inexistente.
