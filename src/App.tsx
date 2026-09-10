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

const moduleCopy: Record<string, { eyebrow: string; title: string; intro: string }> = {
  "Visão geral": {
    eyebrow: "VISÃO GERAL",
    title: "Estado da máquina.",
    intro: "Telemetria local para acompanhar recurso, processo e inicialização. Os números abaixo vêm deste computador.",
  },
  Atividade: {
    eyebrow: "ATIVIDADE",
    title: "Atividade — processos em execução",
    intro: "Leitura local dos processos que mais consomem memória neste momento.",
  },
  Desempenho: {
    eyebrow: "DESEMPENHO",
    title: "Desempenho — recursos em uso",
    intro: "CPU e memória em tempo real, sem dados estimados ou enviados à nuvem.",
  },
  Armazenamento: {
    eyebrow: "ARMAZENAMENTO",
    title: "Armazenamento — capacidade local",
    intro: "Leitura atual do volume C:. O detalhamento por diretórios será adicionado neste módulo.",
  },
  "Inicialização e segurança": {
    eyebrow: "INICIALIZAÇÃO E SEGURANÇA",
    title: "Inicialização — controle local",
    intro: "Inventário do boot do Windows e a opção para registrar o Pulse na sua sessão.",
  },
};

type TopProcess = { name: string; id: number; memoryMb: number; };
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
type StartupRegistration = { enabled: boolean; };

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
  const module = moduleCopy[activeArea];
  const activeIndex = navigationItems.indexOf(activeArea) + 1;
  const memoryPercent = overview && overview.memoryTotalGb > 0 ? Math.round((overview.memoryUsedGb / overview.memoryTotalGb) * 100) : undefined;
  const diskPercent = overview && overview.diskTotalGb > 0 ? Math.round((overview.diskUsedGb / overview.diskTotalGb) * 100) : undefined;

  useEffect(() => {
    invoke<SystemOverview>("get_system_overview").then(setOverview).catch(() => setError("Não foi possível coletar agora. Tente atualizar."));
    invoke<StartupRegistration>("get_startup_registration").then((registration) => setStartupEnabled(registration.enabled)).catch(() => setError("Não foi possível consultar a inicialização do Pulse."));
  }, []);

  async function changeStartupRegistration(enabled: boolean) {
    try {
      const registration = await invoke<StartupRegistration>("set_startup_registration", { enabled });
      setStartupEnabled(registration.enabled);
    } catch {
      setError("O Windows não permitiu alterar a inicialização do Pulse.");
    }
  }

  function goTo(area: string) { setActiveArea(area); }

  function ProcessTable() {
    return overview?.topProcesses.length ? (
      <ol className="process-list">
        {overview.topProcesses.map((process, index) => (
          <li key={process.id}>
            <span className="process-rank">0{index + 1}</span>
            <span className="process-name">{process.name}<small>PID {process.id}</small></span>
            <span>{metric(process.memoryMb, " MB")}</span>
          </li>
        ))}
      </ol>
    ) : <p className="muted">Aguardando a leitura dos processos.</p>;
  }

  function OverviewDashboard() {
    return <section className="dashboard" aria-labelledby="system-summary-title">
      <div className="section-heading">
        <div><p className="eyebrow">LEITURA ATUAL</p><h3 id="system-summary-title">Painel de telemetria local</h3></div>
        <p className="sample-status">{overview ? "AMOSTRA RECEBIDA AGORA" : "AGUARDANDO PRIMEIRA AMOSTRA"}</p>
      </div>
      <div className="metric-grid">
        <button aria-label="Abrir desempenho — CPU" className="metric-card" onClick={() => goTo("Desempenho")} type="button">
          <div className="metric-top"><p>CPU agora</p><span>01</span></div><strong>{metric(overview?.cpuPercent, "%")}</strong><Meter value={overview?.cpuPercent} /><small>USO TOTAL DO PROCESSADOR · ABRIR MÓDULO</small>
        </button>
        <button aria-label="Abrir desempenho — memória" className="metric-card" onClick={() => goTo("Desempenho")} type="button">
          <div className="metric-top"><p>Memória RAM</p><span>02</span></div><strong>{overview ? `${metric(overview.memoryUsedGb, " GB")}` : "—"}</strong><Meter value={memoryPercent} /><small>{overview ? `${metric(overview.memoryTotalGb, " GB")} INSTALADOS · ${memoryPercent ?? "—"}% EM USO` : "AGUARDANDO COLETA"}</small>
        </button>
        <button aria-label="Abrir armazenamento" className="metric-card metric-alert" onClick={() => goTo("Armazenamento")} type="button">
          <div className="metric-top"><p>Disco C:</p><span>03</span></div><strong>{overview ? `${metric(overview.diskUsedGb, " GB")}` : "—"}</strong><Meter value={diskPercent} /><small>{overview ? `${metric(overview.diskTotalGb, " GB")} · ${diskPercent ?? "—"}% OCUPADO` : "AGUARDANDO COLETA"}</small>
        </button>
        <button aria-label="Abrir inicialização e segurança" className="metric-card" onClick={() => goTo("Inicialização e segurança")} type="button">
          <div className="metric-top"><p>Inicialização</p><span>04</span></div><strong>{overview?.startupItemCount ?? "—"}</strong><div className="startup-status">{startupEnabled ? "PULSE REGISTRADO" : "PULSE FORA DO BOOT"}</div><small>ITENS NO INVENTÁRIO DO WINDOWS</small>
        </button>
      </div>
      <div className="detail-grid">
        <section className="panel process-panel" aria-labelledby="processes-title">
          <div className="panel-title"><div><p className="eyebrow">MAIOR CONSUMO</p><h3 id="processes-title">Processos com mais RAM</h3></div><span className="live-dot">AO VIVO</span></div>
          <div className="table-head"><span>PROCESSO</span><span>MEMÓRIA PRIVADA</span></div><ProcessTable />
        </section>
        <section className="panel controls-panel" aria-labelledby="next-signals-title">
          <p className="eyebrow">MODO DE OPERAÇÃO</p><h3 id="next-signals-title">Inicialização e cobertura</h3>
          <label className="startup-toggle"><span><strong>Iniciar com o Windows</strong><small>Registro por usuário no Windows.</small></span><input aria-label="Iniciar com o Windows" checked={startupEnabled} onChange={(event) => void changeStartupRegistration(event.target.checked)} type="checkbox" /></label>
          <ul className="signal-list"><li><span>GPU e temperatura</span><small>DRIVER</small></li><li><span>Arquivos recentes</span><small>MIGRAÇÃO</small></li><li><span>Riscos explicáveis</span><small>PRÓXIMO</small></li></ul>
        </section>
      </div>
    </section>;
  }

  function ModuleDashboard() {
    const title = activeArea === "Atividade" ? "Processos com mais RAM" : activeArea === "Desempenho" ? "CPU e memória" : activeArea === "Armazenamento" ? "Volume C:" : "Inicialização do Windows";
    return <section className="module-dashboard" aria-labelledby="module-data-title">
      <div className="section-heading"><div><p className="eyebrow">LEITURA FOCADA</p><h3 id="module-data-title">{title}</h3></div><button className="back-button" onClick={() => goTo("Visão geral")} type="button">← VISÃO GERAL</button></div>
      {activeArea === "Atividade" && <section className="panel tall-panel"><div className="table-head"><span>PROCESSO</span><span>MEMÓRIA PRIVADA</span></div><ProcessTable /></section>}
      {activeArea === "Desempenho" && <div className="focused-grid"><section className="panel focused-card"><p>CPU AGORA</p><strong>{metric(overview?.cpuPercent, "%")}</strong><Meter value={overview?.cpuPercent} /></section><section className="panel focused-card"><p>MEMÓRIA RAM</p><strong>{overview ? `${metric(overview.memoryUsedGb, " GB")} / ${metric(overview.memoryTotalGb, " GB")}` : "—"}</strong><Meter value={memoryPercent} /></section></div>}
      {activeArea === "Armazenamento" && <section className="panel focused-card storage-card"><p>CAPACIDADE UTILIZADA</p><strong>{overview ? `${metric(overview.diskUsedGb, " GB")} / ${metric(overview.diskTotalGb, " GB")}` : "—"}</strong><Meter value={diskPercent} /><small>MAPA POR DIRETÓRIOS AINDA NÃO DISPONÍVEL</small></section>}
      {activeArea === "Inicialização e segurança" && <section className="panel focused-card startup-card"><p>ITENS DETECTADOS</p><strong>{overview?.startupItemCount ?? "—"}</strong><label className="startup-toggle"><span><strong>Iniciar com o Windows</strong><small>Registro por usuário no Windows.</small></span><input aria-label="Iniciar com o Windows" checked={startupEnabled} onChange={(event) => void changeStartupRegistration(event.target.checked)} type="checkbox" /></label></section>}
    </section>;
  }

  return <main className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark" aria-hidden="true"><i /><i /><i /></span><div><h1>Pulse</h1><p>Windows observability</p></div></div>
      <nav aria-label="Navegação principal"><p className="nav-label">MÓDULOS</p><ul>{navigationItems.map((item, index) => <li key={item}><button aria-current={activeArea === item ? "page" : undefined} className={activeArea === item ? "nav-item active" : "nav-item"} onClick={() => goTo(item)} type="button"><span className="nav-index">0{index + 1}</span><span className="nav-title">{item}</span></button></li>)}</ul></nav>
      <div className="sidebar-foot"><span className="connection-state">LOCAL</span><p>Sem nuvem<br />Sem conta</p></div>
    </aside>
    <section className="content" aria-labelledby="overview-title">
      <header className="command-bar"><div><span>HOST</span><b>ESTAÇÃO LOCAL</b></div><div><span>COLETA</span><b className={overview ? "state-good" : "state-pending"}>{overview ? "ATIVA" : "INICIANDO"}</b></div><button className="refresh-button" onClick={() => window.location.reload()} type="button">RELER DADOS <span>↻</span></button></header>
      <div className="overview-heading"><div><p className="eyebrow">{module.eyebrow} / 0{activeIndex}</p><h2 id="overview-title">{module.title}</h2></div><p className="intro">{module.intro}</p></div>
      <div className="workspace" key={activeArea}>{activeArea === "Visão geral" ? <OverviewDashboard /> : <ModuleDashboard />}{error && <p className="collection-error" role="status">{error}</p>}{overview && <footer className="source-note"><span>FONTE / {overview.source}</span><span>ESCOPO / DISPOSITIVO ATUAL</span></footer>}</div>
    </section>
  </main>;
}

export default App;
