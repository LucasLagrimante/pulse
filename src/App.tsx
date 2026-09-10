import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import "./App.css";

const navigationItems = [
  "Visão geral",
  "Atividade",
  "Desempenho",
  "Armazenamento",
  "Inicialização e segurança",
];

type TopProcess = {
  name: string;
  id: number;
  memoryMb: number;
};

type SystemOverview = {
  cpuPercent: number;
  memoryUsedGb: number;
  memoryTotalGb: number;
  diskUsedGb: number;
  diskTotalGb: number;
  startupItemCount: number;
  topProcesses: TopProcess[];
  sampledAtUnixMs: number;
  source: string;
};

function metric(value: number | undefined, suffix: string) {
  return value === undefined ? "—" : `${value.toLocaleString("pt-BR", { maximumFractionDigits: 1 })}${suffix}`;
}

function App() {
  const [activeArea, setActiveArea] = useState(navigationItems[0]);
  const [overview, setOverview] = useState<SystemOverview>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    invoke<SystemOverview>("get_system_overview")
      .then(setOverview)
      .catch(() => setError("Não foi possível coletar agora. Tente atualizar."));
  }, []);

  const memoryPercent = overview && overview.memoryTotalGb > 0
    ? Math.round((overview.memoryUsedGb / overview.memoryTotalGb) * 100)
    : undefined;
  const diskPercent = overview && overview.diskTotalGb > 0
    ? Math.round((overview.diskUsedGb / overview.diskTotalGb) * 100)
    : undefined;

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">P</span>
          <div>
            <h1>Pulse</h1>
            <p>Saúde do seu Windows</p>
          </div>
        </div>

        <nav aria-label="Navegação principal">
          <ul>
            {navigationItems.map((item) => (
              <li key={item}>
                <button
                  aria-current={activeArea === item ? "page" : undefined}
                  className={activeArea === item ? "nav-item active" : "nav-item"}
                  onClick={() => setActiveArea(item)}
                  type="button"
                >
                  {item}
                </button>
              </li>
            ))}
          </ul>
        </nav>

        <p className="local-first">Dados locais · sem conta</p>
      </aside>

      <section className="content" aria-labelledby="overview-title">
        <div className="overview-heading">
          <div>
            <p className="eyebrow">{activeArea.toUpperCase()}</p>
            <h2 id="overview-title">Seu PC, explicado.</h2>
            <p className="intro">Uma leitura rápida do que está consumindo recursos agora, sem enviar seus dados para a nuvem.</p>
          </div>
          <button className="refresh-button" onClick={() => window.location.reload()} type="button">Atualizar</button>
        </div>

        <section className="dashboard" aria-labelledby="system-summary-title">
          <div className="section-heading">
            <div>
              <p className="eyebrow">AGORA</p>
              <h3 id="system-summary-title">Resumo do sistema</h3>
            </div>
            <p className="sample-status">{overview ? "Atualizado agora" : "Coletando dados locais…"}</p>
          </div>

          <div className="metric-grid">
            <article className="metric-card accent-cyan">
              <p>CPU agora</p>
              <strong>{metric(overview?.cpuPercent, "%")}</strong>
              <span>Uso total do processador</span>
            </article>
            <article className="metric-card accent-blue">
              <p>Memória RAM</p>
              <strong>{overview ? `${metric(overview.memoryUsedGb, " GB")} / ${metric(overview.memoryTotalGb, " GB")}` : "—"}</strong>
              <span>{memoryPercent === undefined ? "Aguardando coleta" : `${memoryPercent}% em uso`}</span>
            </article>
            <article className="metric-card accent-violet">
              <p>Disco C:</p>
              <strong>{overview ? `${metric(overview.diskUsedGb, " GB")} / ${metric(overview.diskTotalGb, " GB")}` : "—"}</strong>
              <span>{diskPercent === undefined ? "Aguardando coleta" : `${diskPercent}% ocupado`}</span>
            </article>
            <article className="metric-card accent-amber">
              <p>Itens na inicialização</p>
              <strong>{overview?.startupItemCount ?? "—"}</strong>
              <span>Inventário inicial do Windows</span>
            </article>
          </div>

          <div className="detail-grid">
            <section className="panel" aria-labelledby="processes-title">
              <div className="panel-title">
                <div>
                  <p className="eyebrow">MEMÓRIA</p>
                  <h3 id="processes-title">Processos com mais RAM</h3>
                </div>
                <span className="live-dot">Ao vivo</span>
              </div>
              {overview?.topProcesses.length ? (
                <ol className="process-list">
                  {overview.topProcesses.map((process) => (
                    <li key={process.id}>
                      <span className="process-name">{process.name}</span>
                      <span>{metric(process.memoryMb, " MB")}</span>
                    </li>
                  ))}
                </ol>
              ) : (
                <p className="muted">Aguardando a leitura dos processos.</p>
              )}
            </section>

            <section className="panel" aria-labelledby="next-signals-title">
              <p className="eyebrow">EM BREVE</p>
              <h3 id="next-signals-title">Próximos sinais</h3>
              <ul className="signal-list">
                <li><span>GPU e temperatura</span><small>Depende do driver</small></li>
                <li><span>Arquivos recentes</span><small>Em migração</small></li>
                <li><span>Riscos explicáveis</span><small>Sem ações automáticas</small></li>
              </ul>
            </section>
          </div>

          {error && <p className="collection-error" role="status">{error}</p>}
          {overview && <p className="source-note">Fonte: {overview.source} · somente neste dispositivo</p>}
        </section>
      </section>
    </main>
  );
}

export default App;
