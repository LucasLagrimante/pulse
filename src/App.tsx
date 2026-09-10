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

type StartupRegistration = {
  enabled: boolean;
};

function metric(value: number | undefined, suffix: string) {
  return value === undefined ? "—" : `${value.toLocaleString("pt-BR", { maximumFractionDigits: 1 })}${suffix}`;
}

function Meter({ value }: { value?: number }) {
  const safeValue = Math.max(0, Math.min(100, value ?? 0));
  return <span aria-hidden="true" className="meter"><i style={{ width: `${safeValue}%` }} /></span>;
}

function App() {
  const [activeArea, setActiveArea] = useState(navigationItems[0]);
  const [overview, setOverview] = useState<SystemOverview>();
  const [startupEnabled, setStartupEnabled] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    invoke<SystemOverview>("get_system_overview")
      .then(setOverview)
      .catch(() => setError("Não foi possível coletar agora. Tente atualizar."));
    invoke<StartupRegistration>("get_startup_registration")
      .then((registration) => setStartupEnabled(registration.enabled))
      .catch(() => setError("Não foi possível consultar a inicialização do Pulse."));
  }, []);

  async function changeStartupRegistration(enabled: boolean) {
    try {
      const registration = await invoke<StartupRegistration>("set_startup_registration", { enabled });
      setStartupEnabled(registration.enabled);
    } catch {
      setError("O Windows não permitiu alterar a inicialização do Pulse.");
    }
  }

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
          <span className="brand-mark" aria-hidden="true"><i /><i /><i /></span>
          <div>
            <h1>Pulse</h1>
            <p>Windows observability</p>
          </div>
        </div>

        <nav aria-label="Navegação principal">
          <p className="nav-label">MÓDULOS</p>
          <ul>
            {navigationItems.map((item, index) => (
              <li key={item}>
                <button
                  aria-current={activeArea === item ? "page" : undefined}
                  className={activeArea === item ? "nav-item active" : "nav-item"}
                  onClick={() => setActiveArea(item)}
                  type="button"
                >
                  <span className="nav-index">0{index + 1}</span>{item}
                </button>
              </li>
            ))}
          </ul>
        </nav>

        <div className="sidebar-foot">
          <span className="connection-state">LOCAL</span>
          <p>Sem nuvem<br />Sem conta</p>
        </div>
      </aside>

      <section className="content" aria-labelledby="overview-title">
        <header className="command-bar">
          <div><span>HOST</span><b>ESTAÇÃO LOCAL</b></div>
          <div><span>COLETA</span><b className={overview ? "state-good" : "state-pending"}>{overview ? "ATIVA" : "INICIANDO"}</b></div>
          <button className="refresh-button" onClick={() => window.location.reload()} type="button">RELER DADOS <span>↻</span></button>
        </header>

        <div className="overview-heading">
          <div>
            <p className="eyebrow">{activeArea.toUpperCase()} / 01</p>
            <h2 id="overview-title">Estado da máquina.<br /><em>Sem interpretação vazia.</em></h2>
          </div>
          <p className="intro">Telemetria local para acompanhar recurso, processo e inicialização. Os números abaixo vêm deste computador.</p>
        </div>

        <section className="dashboard" aria-labelledby="system-summary-title">
          <div className="section-heading">
            <div>
              <p className="eyebrow">LEITURA ATUAL</p>
              <h3 id="system-summary-title">Painel de telemetria local</h3>
            </div>
            <p className="sample-status">{overview ? "AMOSTRA RECEBIDA AGORA" : "AGUARDANDO PRIMEIRA AMOSTRA"}</p>
          </div>

          <div className="metric-grid">
            <article className="metric-card">
              <div className="metric-top"><p>CPU agora</p><span>01</span></div>
              <strong>{metric(overview?.cpuPercent, "%")}</strong>
              <Meter value={overview?.cpuPercent} />
              <small>USO TOTAL DO PROCESSADOR</small>
            </article>
            <article className="metric-card">
              <div className="metric-top"><p>Memória RAM</p><span>02</span></div>
              <strong>{overview ? `${metric(overview.memoryUsedGb, " GB")}` : "—"}</strong>
              <Meter value={memoryPercent} />
              <small>{overview ? `${metric(overview.memoryTotalGb, " GB")} INSTALADOS · ${memoryPercent ?? "—"}% EM USO` : "AGUARDANDO COLETA"}</small>
            </article>
            <article className="metric-card metric-alert">
              <div className="metric-top"><p>Disco C:</p><span>03</span></div>
              <strong>{overview ? `${metric(overview.diskUsedGb, " GB")}` : "—"}</strong>
              <Meter value={diskPercent} />
              <small>{overview ? `${metric(overview.diskTotalGb, " GB")} · ${diskPercent ?? "—"}% OCUPADO` : "AGUARDANDO COLETA"}</small>
            </article>
            <article className="metric-card">
              <div className="metric-top"><p>Inicialização</p><span>04</span></div>
              <strong>{overview?.startupItemCount ?? "—"}</strong>
              <div className="startup-status">{startupEnabled ? "PULSE REGISTRADO" : "PULSE FORA DO BOOT"}</div>
              <small>ITENS NO INVENTÁRIO DO WINDOWS</small>
            </article>
          </div>

          <div className="detail-grid">
            <section className="panel process-panel" aria-labelledby="processes-title">
              <div className="panel-title">
                <div>
                  <p className="eyebrow">MAIOR CONSUMO</p>
                  <h3 id="processes-title">Processos com mais RAM</h3>
                </div>
                <span className="live-dot">AO VIVO</span>
              </div>
              <div className="table-head"><span>PROCESSO</span><span>MEMÓRIA PRIVADA</span></div>
              {overview?.topProcesses.length ? (
                <ol className="process-list">
                  {overview.topProcesses.map((process, index) => (
                    <li key={process.id}>
                      <span className="process-rank">0{index + 1}</span>
                      <span className="process-name">{process.name}<small>PID {process.id}</small></span>
                      <span>{metric(process.memoryMb, " MB")}</span>
                    </li>
                  ))}
                </ol>
              ) : <p className="muted">Aguardando a leitura dos processos.</p>}
            </section>

            <section className="panel controls-panel" aria-labelledby="next-signals-title">
              <p className="eyebrow">MODO DE OPERAÇÃO</p>
              <h3 id="next-signals-title">Inicialização e cobertura</h3>
              <label className="startup-toggle">
                <span><strong>Iniciar com o Windows</strong><small>Registro por usuário no Windows.</small></span>
                <input aria-label="Iniciar com o Windows" checked={startupEnabled} onChange={(event) => void changeStartupRegistration(event.target.checked)} type="checkbox" />
              </label>
              <ul className="signal-list">
                <li><span>GPU e temperatura</span><small>DRIVER</small></li>
                <li><span>Arquivos recentes</span><small>MIGRAÇÃO</small></li>
                <li><span>Riscos explicáveis</span><small>PRÓXIMO</small></li>
              </ul>
            </section>
          </div>

          {error && <p className="collection-error" role="status">{error}</p>}
          {overview && <footer className="source-note"><span>FONTE / {overview.source}</span><span>ESCOPO / DISPOSITIVO ATUAL</span></footer>}
        </section>
      </section>
    </main>
  );
}

export default App;
