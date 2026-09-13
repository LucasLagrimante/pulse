# Um indicador gráfico por card de telemetria

- CPU e Memória mantêm apenas o sparkline, pois ele mostra o valor atual no contexto do histórico recente.
- Disco mantém apenas a barra de capacidade com os rótulos de espaço usado e livre.
- Não renderizar um `Meter` adicional nos cards da Visão Geral, de Desempenho ou do tray: ele repete a mesma proporção sem acrescentar contexto.
