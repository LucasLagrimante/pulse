import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Component, type ErrorInfo, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import "./App.css";
import "./identity.css";
import { PulseMark, ModuleIcon, PulseLoading, FileIcon } from "./Identity";
import {
  DetailedResourceChart,
  HardwareGaugeBar,
  MiniCapacityBar,
  SparklineChart,
} from "./Charts";
import { DiskAnalyzer } from "./DiskAnalyzer";
import { DiskCleanup } from "./DiskCleanup";

const navigationItems = [
  "Visão geral",
  "Atividade",
  "Desempenho",
  "Armazenamento",
  "Inicialização e segurança",
  "Debug",
];
const INITIAL_BOOT_MINIMUM_MS = 2200;
const INITIAL_CHART_SKELETON_MS = INITIAL_BOOT_MINIMUM_MS + 850;
const INITIAL_CHART_SAMPLE_DELAY_MS = 3600;
const MIN_CHART_HISTORY_SAMPLES = 2;
const TELEMETRY_POLL_INTERVAL_MS = 5000;
const TOP_PROCESS_LIMIT = 25;

function useWindowForeground(): boolean | null {
  const [isForeground, setIsForeground] = useState<boolean | null>(null);

  useEffect(() => {
    let disposed = false;
    let receivedNativeEvent = false;
    let unlisten: (() => void) | undefined;

    listen<boolean>("pulse://foreground-changed", event => {
      receivedNativeEvent = true;
      setIsForeground(event.payload === true);
    })
      .then(cleanup => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      })
      .catch(() => undefined);

    invoke<boolean>("is_current_window_foreground")
      .then(active => {
        if (!disposed && !receivedNativeEvent) {
          // O fallback mantém os testes/web preview funcionais; no Tauri o comando sempre retorna boolean.
          setIsForeground(typeof active === "boolean" ? active : true);
        }
      })
      .catch(() => {
        if (!disposed && !receivedNativeEvent) setIsForeground(true);
      });

    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  return isForeground;
}

export type TopProcess = {
  name: string;
  id: number;
  memoryMb: number;
  processIds?: number[];
  processCount?: number;
  displayName?: string | null;
  publisher?: string | null;
};

function isWslHostProcess(process: TopProcess) {
  return process.name.replace(/\.exe$/i, "").toLowerCase() === "vmmemwsl";
}
export type SystemOverview = {
  cpuPercent: number | null;
  memoryUsedGb: number | null;
  memoryTotalGb: number | null;
  diskUsedGb: number | null;
  diskTotalGb: number | null;
  startupItemCount: number;
  topProcesses: TopProcess[];
  sampledAtUnixMs: number;
  source: string;
};
export type RecentFile = {
  path: string;
  name: string;
  category: string;
  sizeBytes: number;
  modifiedAtUnixMs: number;
};
export type Capability<T> = { value?: T | null; unavailableReason?: string | null };
export type Performance = {
  gpu?: Capability<string>;
  vramBytes?: Capability<number>;
  temperatureCelsius?: Capability<number>;
};
export type StartupItem = {
  name: string;
  origin: string;
  command: string;
  enabled: boolean;
  canChange: boolean;
  unavailableReason?: string;
  impact?: string;
  startupTimeMs?: number | null;
};
export type DebugEntry = {
  sequence?: number;
  timestampUnixMs: number;
  operation: string;
  category?: string;
  source?: string;
  durationMs: number;
  outcome: string;
  severity?: string;
  message: string;
};
export type DebugAuditInfo = {
  enabled: boolean;
  sessionStartedUnixMs: number;
  entryCount: number;
  retentionLimit: number;
  applicationVersion: string;
  platform: string;
  persistence: string;
  dataResidency: string;
};
export type StartupRegistration = { enabled: boolean };

const copy: Record<string, { eyebrow: string; title: string; intro: string }> = {
  "Visão geral": {
    eyebrow: "VISÃO GERAL",
    title: "Estado da máquina.",
    intro: "Telemetria local para acompanhar recursos, processos, arquivos e inicialização.",
  },
  Atividade: {
    eyebrow: "ATIVIDADE",
    title: "Atividade — processos em execução",
    intro: "Leitura local dos processos que mais consomem memória neste momento.",
  },
  Desempenho: {
    eyebrow: "DESEMPENHO",
    title: "Desempenho — recursos em uso",
    intro: "CPU, memória e capacidades de hardware sem dados estimados.",
  },
  Armazenamento: {
    eyebrow: "ARMAZENAMENTO",
    title: "Armazenamento — capacidade local",
    intro: "Analise a ocupação, revise arquivos recentes e recupere espaço com uma limpeza local e controlada.",
  },
  "Inicialização e segurança": {
    eyebrow: "INICIALIZAÇÃO E SEGURANÇA",
    title: "Inicialização — controle local",
    intro: "Inventário do boot do Windows e alterações somente onde suportadas.",
  },
  Debug: {
    eyebrow: "DEBUG / AUDITORIA",
    title: "Rastro técnico do Pulse.",
    intro: "Operações, duração, origem e falhas expostas em uma trilha local e verificável.",
  },
  Informações: {
    eyebrow: "INFORMAÇÕES",
    title: "Sobre o Pulse.",
    intro: "Identidade, autoria e canais oficiais deste aplicativo local para Windows.",
  },
};

export const metric = (v: number | undefined | null, suffix = ""): string => {
  if (v === undefined || v === null || typeof v !== "number" || Number.isNaN(v)) {
    return "—";
  }
  return `${v.toLocaleString("pt-BR", { maximumFractionDigits: 1 })}${suffix}`;
};

const debugCategoryLabel = (category?: string) => ({
  telemetry: "Telemetria",
  storage: "Armazenamento",
  interface: "Interface",
  system_action: "Ação no sistema",
  application: "Aplicativo",
}[category ?? "application"] ?? category ?? "Aplicativo");

const debugOperationLabel = (operation: string) => ({
  system_overview: "Coleta da visão geral",
  performance_details: "Leitura de desempenho",
  recent_files: "Consulta de arquivos recentes",
  refresh_recent_files: "Atualização de arquivos recentes",
  startup_inventory: "Inventário de inicialização",
  startup_item_changed: "Alteração na inicialização",
  terminate_process: "Encerramento de processo",
  interface_render: "Renderização da interface",
  debug_mode_changed: "Estado da auditoria",
}[operation] ?? operation.replace(/_/g, " "));

export type DiskCapacityHealth = "normal" | "warning" | "critical" | "unknown";

export function getDiskCapacityHealth(percent: number | undefined | null): DiskCapacityHealth {
  if (typeof percent !== "number" || !Number.isFinite(percent)) return "unknown";
  if (percent >= 95) return "critical";
  if (percent >= 85) return "warning";
  return "normal";
}

export function diskCapacityColor(health: DiskCapacityHealth): string {
  if (health === "critical") return "#e36a45";
  if (health === "warning") return "#dda76d";
  return "#8da9bf";
}

function DiskHealthBadge({ health }: { health: DiskCapacityHealth }) {
  if (health !== "warning" && health !== "critical") return null;
  const label = health === "critical" ? "CRÍTICO" : "ATENÇÃO";
  return (
    <span className={`disk-health-badge ${health}`} title={health === "critical" ? "Espaço livre crítico" : "Armazenamento quase cheio"}>
      <i aria-hidden="true" />
      {label}
    </span>
  );
}

export function formatFileSize(bytes: number | undefined | null): string {
  if (!bytes || bytes <= 0 || typeof bytes !== "number" || Number.isNaN(bytes)) {
    return "0 B";
  }
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1).replace(".", ",")} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1).replace(".", ",")} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1).replace(".", ",")} GB`;
}

export function formatFileDate(timestampMs: number | undefined | null): string {
  if (!timestampMs || timestampMs <= 0 || typeof timestampMs !== "number" || Number.isNaN(timestampMs)) {
    return "Data indisponível";
  }
  const date = new Date(timestampMs);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMinutes = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  const timeStr = date.toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });

  if (diffMinutes < 1) return "Agora mesmo";
  if (diffMinutes < 60) return `Há ${diffMinutes} min`;
  if (diffHours < 24 && date.getDate() === now.getDate()) return `Hoje às ${timeStr}`;
  if (diffDays === 1 || (diffDays === 0 && date.getDate() !== now.getDate())) return `Ontem às ${timeStr}`;
  if (diffDays < 7) {
    const dayName = date.toLocaleDateString("pt-BR", { weekday: "short" });
    return `${dayName} às ${timeStr}`;
  }

  return date.toLocaleDateString("pt-BR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function formatStartupDuration(ms: number | undefined | null): string {
  if (ms == null || ms <= 0 || typeof ms !== "number" || Number.isNaN(ms)) {
    return "";
  }
  if (ms >= 1000) {
    return `${(ms / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} s`;
  }
  return `${ms} ms`;
}

type CrashBoundaryProps = { children: ReactNode; onRecover: () => void };
type CrashBoundaryState = { message?: string; details?: string };

export class CrashBoundary extends Component<CrashBoundaryProps, CrashBoundaryState> {
  state: CrashBoundaryState = {};

  static getDerivedStateFromError(error: Error): CrashBoundaryState {
    return {
      message: error.message || "Falha inesperada ao renderizar este módulo.",
      details: error.stack,
    };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    void invoke("record_client_error", {
      message: `${error.name}: ${error.message}`,
      componentStack: info.componentStack ?? "",
    }).catch(() => undefined);
  }

  render() {
    if (this.state.message) {
      return (
        <main className="crash-screen" role="alert">
          <p className="eyebrow">RECUPERAÇÃO LOCAL</p>
          <h1>Este módulo não pôde ser exibido.</h1>
          <p className="crash-message">{this.state.message}</p>
          {this.state.details && <pre className="crash-details">{this.state.details}</pre>}
          <div className="crash-actions">
            <button onClick={this.props.onRecover}>VOLTAR À VISÃO GERAL</button>
            <button className="crash-secondary" onClick={() => window.location.reload()}>
              REABRIR INTERFACE
            </button>
            <button className="crash-secondary" onClick={() => window.close()}>
              FECHAR
            </button>
          </div>
        </main>
      );
    }
    return this.props.children;
  }
}

export function ProcessList({
  processes,
  onTerminate,
  terminatingId,
}: {
  processes: TopProcess[];
  onTerminate?: (process: TopProcess) => void;
  terminatingId?: number | null;
}) {
  return (
    <ol className="process-list">
      {processes.slice(0, TOP_PROCESS_LIMIT).map((p, i) => (
        <li key={`${p.id}-${p.name}`}>
          <span className="process-rank">{String(i + 1).padStart(2, "0")}</span>
          <span className="process-name" title={[p.displayName || p.name, p.name, p.publisher].filter(Boolean).join(" · ")}>
            {p.displayName || p.name}
            <small>
              {[p.name, p.publisher].filter(Boolean).join(" · ") || `PID ${p.id}`}
              {(p.processCount ?? 1) > 1 ? ` · ${p.processCount} processos` : ""}
            </small>
          </span>
          <span className="process-memory">{metric(p.memoryMb, " MB")}</span>
          {onTerminate && (
            <button
              type="button"
              className="process-close-button"
              aria-label={`Fechar ${p.displayName || p.name}`}
              disabled={terminatingId === p.id || p.name.toLocaleLowerCase("pt-BR").startsWith("pulse")}
              onClick={() => onTerminate(p)}
            >
              {terminatingId === p.id ? "FECHANDO…" : "FECHAR"}
            </button>
          )}
        </li>
      ))}
    </ol>
  );
}

export type ToastStatus = {
  type: "loading" | "success" | "info" | "error";
  message: string;
};

export function ToastNotification({
  toast,
  onDismiss,
}: {
  toast: ToastStatus | null;
  onDismiss: () => void;
}) {
  if (!toast) return null;

  return (
    <div
      className={`pulse-toast pulse-toast-${toast.type}`}
      role="status"
      aria-live="polite"
      onClick={onDismiss}
    >
      <div className="toast-icon">
        {toast.type === "loading" && <PulseMark animated />}
        {toast.type === "success" && <span className="toast-check" aria-hidden="true">✓</span>}
        {toast.type === "info" && <span className="toast-info-mark" aria-hidden="true">ℹ</span>}
        {toast.type === "error" && <span className="toast-err-mark" aria-hidden="true">✕</span>}
      </div>
      <div className="toast-content">
        <span className="toast-title">
          {toast.type === "loading" && "BUSCANDO"}
          {toast.type === "success" && "CONCLUÍDO"}
          {toast.type === "info" && "INFORMAÇÃO"}
          {toast.type === "error" && "ATENÇÃO"}
        </span>
        <span className="toast-message">{toast.message}</span>
      </div>
      <button
        className="toast-close"
        onClick={e => {
          e.stopPropagation();
          onDismiss();
        }}
        aria-label="Fechar notificação"
      >
        ×
      </button>
    </div>
  );
}

export function ScanFoldersBar({
  customFolders,
  newFolderPath,
  loading,
  onNewFolderPathChange,
  onAddFolder,
  onRemoveFolder,
}: {
  customFolders: string[];
  newFolderPath: string;
  loading: boolean;
  onNewFolderPathChange: (path: string) => void;
  onAddFolder: (path?: string) => void;
  onRemoveFolder: (path: string) => void;
}) {
  const [showConfig, setShowConfig] = useState(true);

  return (
    <section className="panel scan-folders-panel" aria-labelledby="scan-folders-title">
      <div className="panel-title">
        <div>
          <p className="eyebrow">ESCOPO DE VARREDURA</p>
          <h3 id="scan-folders-title">Pastas monitoradas ({6 + customFolders.length})</h3>
        </div>
        <button
          type="button"
          className="text-button"
          onClick={() => setShowConfig(!showConfig)}
          aria-expanded={showConfig}
        >
          {showConfig ? "OCULTAR PASTAS" : "CONFIGURAR PASTAS"}
        </button>
      </div>

      {showConfig && (
        <div className="scan-folders-body">
          <div className="scan-folders-list">
            <div className="scan-folder-chip default-chip" title="Pastas padrão do perfil Windows">
              <FileIcon folder />
              <span className="folder-name">Padrão do Usuário (Downloads, Documentos, Desktop, Imagens, Vídeos, Músicas)</span>
              <span className="folder-badge">PADRÃO</span>
            </div>

            {customFolders.map(folder => (
              <div key={folder} className="scan-folder-chip custom-chip" title={folder}>
                <ModuleIcon index={3} />
                <span className="folder-name font-mono">{folder}</span>
                <span className="folder-badge custom-badge">PERSONALIZADA</span>
                <button
                  type="button"
                  className="folder-remove-btn"
                  title={`Remover pasta ${folder}`}
                  aria-label={`Remover pasta ${folder}`}
                  disabled={loading}
                  onClick={() => onRemoveFolder(folder)}
                >
                  ✕
                </button>
              </div>
            ))}
          </div>

          <form
            className="add-folder-form"
            onSubmit={e => {
              e.preventDefault();
              onAddFolder();
            }}
          >
            <input
              type="text"
              aria-label="Caminho da nova pasta para vasculhar"
              placeholder="Ex: D:\Images\Screenshots ou D:\Projetos"
              value={newFolderPath}
              onChange={e => onNewFolderPathChange(e.target.value)}
              className="add-folder-input font-mono"
              disabled={loading}
            />
            <button
              type="submit"
              className="btn-primary add-folder-btn"
              disabled={loading || !newFolderPath.trim()}
            >
              + ADICIONAR PASTA
            </button>
          </form>
        </div>
      )}
    </section>
  );
}

export function FileList({
  files,
  busy,
  fileQuery,
  fileCategory,
  fileSort,
  onQueryChange,
  onCategoryChange,
  onSortChange,
  onRefresh,
  onOpen,
}: {
  files: RecentFile[];
  busy: boolean;
  fileQuery: string;
  fileCategory: string;
  fileSort: string;
  onQueryChange: (query: string) => void;
  onCategoryChange: (category: string) => void;
  onSortChange: (sort: string) => void;
  onRefresh: () => void;
  onOpen: (file: RecentFile, location?: boolean) => void;
}) {
  return (
    <section className="panel recent-panel" aria-labelledby="recent-title">
      <div className="panel-title">
        <div>
          <p className="eyebrow">ESCOPO LOCAL</p>
          <h3 id="recent-title">Arquivos recentes</h3>
        </div>
        <button className="text-button" onClick={onRefresh} disabled={busy}>
          {busy ? "ATUALIZANDO…" : "RELER"}
        </button>
      </div>
      <div className="file-tools">
        <input
          aria-label="Buscar arquivos"
          placeholder="Buscar por nome ou caminho"
          value={fileQuery}
          onChange={e => onQueryChange(e.target.value)}
        />
        <select
          aria-label="Filtrar categoria"
          value={fileCategory}
          onChange={e => onCategoryChange(e.target.value)}
        >
          <option value="">Todas as categorias</option>
          <option value="document">Documentos</option>
          <option value="image">Imagens</option>
          <option value="archive">Arquivos compactados</option>
          <option value="code">Código-fonte</option>
          <option value="media">Mídia</option>
        </select>
        <select
          aria-label="Ordenar arquivos"
          value={fileSort}
          onChange={e => onSortChange(e.target.value)}
          className="sort-select"
        >
          <option value="recent">Mais recentes</option>
          <option value="oldest">Mais antigos</option>
          <option value="name">Nome (A-Z)</option>
          <option value="size">Maior tamanho</option>
        </select>
      </div>
      <div className="recent-list">
        {busy ? (
          <div className="skeleton-file-list" aria-label="Carregando arquivos recentes…">
            {[1, 2, 3, 4, 5].map(i => (
              <div className="file-row skeleton-row" key={i}>
                <div className="file-main">
                  <div className="skeleton-line skeleton-line-title" />
                  <div className="file-meta">
                    <div className="skeleton-line skeleton-line-tag" />
                    <div className="skeleton-line skeleton-line-size" />
                  </div>
                </div>
                <div className="file-date-block">
                  <div className="skeleton-line skeleton-line-date" />
                </div>
                <div className="file-actions">
                  <div className="skeleton-line skeleton-line-btn" />
                </div>
              </div>
            ))}
          </div>
        ) : files.length ? (
          files.map(file => {
            const formattedDate = formatFileDate(file.modifiedAtUnixMs);
            const fullDate = new Date(file.modifiedAtUnixMs).toLocaleString("pt-BR");
            return (
              <div className="file-row" key={file.path}>
                <div className="file-main">
                  <span className="file-name" title={file.path}>
                    {file.name}
                  </span>
                  <div className="file-meta">
                    <span className="file-category-tag">{file.category}</span>
                    <span className="file-size">{formatFileSize(file.sizeBytes)}</span>
                  </div>
                </div>
                <div className="file-date-block" title={`Modificado em: ${fullDate}`}>
                  <span className="file-date-label">MODIFICAÇÃO</span>
                  <time className="file-date-value" dateTime={new Date(file.modifiedAtUnixMs).toISOString()}>
                    {formattedDate}
                  </time>
                </div>
                <div className="file-actions">
                  <button onClick={() => onOpen(file)}>Abrir</button>
                  <button onClick={() => onOpen(file, true)}>Localização</button>
                </div>
              </div>
            );
          })
        ) : (
          <p className="muted">Nenhum arquivo encontrado.</p>
        )}
      </div>
    </section>
  );
}

export function TrayFlyout({
  onOpenMain,
  isForeground = true,
}: {
  onOpenMain: () => void;
  isForeground?: boolean | null;
}) {
  const [overview, setOverview] = useState<SystemOverview>();
  const [processToClose, setProcessToClose] = useState<TopProcess | null>(null);
  const [terminatingId, setTerminatingId] = useState<number | null>(null);
  const [processError, setProcessError] = useState("");
  const [cpuHistory, setCpuHistory] = useState<number[]>([]);
  const [ramHistory, setRamHistory] = useState<number[]>([]);

  const loadData = async () => {
    try {
      const result = await invoke<SystemOverview>("get_system_overview");
      if (result && Array.isArray(result.topProcesses)) {
        setOverview(previous => ({
          ...result,
          topProcesses: result.topProcesses.length > 0 ? result.topProcesses : (previous?.topProcesses ?? []),
        }));
        const cpu =
          typeof result.cpuPercent === "number" && !Number.isNaN(result.cpuPercent)
            ? Math.max(0, Math.min(100, result.cpuPercent))
            : Number.NaN;
        const ram =
          typeof result.memoryTotalGb === "number" && result.memoryTotalGb > 0 && typeof result.memoryUsedGb === "number"
            ? Math.round((result.memoryUsedGb / result.memoryTotalGb) * 100)
            : Number.NaN;
        setCpuHistory(prev =>
          [...prev, cpu].slice(-20),
        );
        setRamHistory(prev =>
          [...prev, ram].slice(-20),
        );
      }
    } catch {
      // Continua silencioso no flyout
    }
  };

  useEffect(() => {
    if (isForeground !== true) return;

    void loadData();
    const timer = window.setInterval(() => void loadData(), TELEMETRY_POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [isForeground]);

  const memoryPercent =
    overview && typeof overview.memoryTotalGb === "number" && overview.memoryTotalGb > 0 && typeof overview.memoryUsedGb === "number"
      ? Math.round((overview.memoryUsedGb / overview.memoryTotalGb) * 100)
      : undefined;

  const diskPercent =
    overview && typeof overview.diskTotalGb === "number" && overview.diskTotalGb > 0 && typeof overview.diskUsedGb === "number"
      ? Math.round((overview.diskUsedGb / overview.diskTotalGb) * 100)
      : undefined;
  const diskHealth = getDiskCapacityHealth(diskPercent);

  const handleClose = async () => {
    try {
      await invoke("hide_tray_flyout");
    } catch {
      // Ignora erro
    }
  };

  const handleExit = async () => {
    try {
      await invoke("exit_app");
    } catch {
      window.close();
    }
  };

  const confirmTerminateProcess = async () => {
    if (!processToClose) return;
    setTerminatingId(processToClose.id);
    setProcessError("");
    try {
      await invoke("terminate_process", { id: processToClose.id });
      setOverview(previous => previous ? {
        ...previous,
        topProcesses: previous.topProcesses.filter(process => process.id !== processToClose.id),
      } : previous);
      setProcessToClose(null);
      void loadData();
    } catch (error) {
      setProcessError(typeof error === "string" ? error : "O Windows não permitiu fechar este aplicativo.");
    } finally {
      setTerminatingId(null);
    }
  };

  return (
    <aside className="tray-flyout-container" role="complementary" aria-label="Painel rápido do tray">
      <header className="flyout-header">
        <div className="flyout-brand">
          <PulseMark />
          <div>
            <h2>Pulse</h2>
            <p>PAINEL RÁPIDO</p>
          </div>
        </div>
        <button
          aria-label="Fechar painel rápido"
          className="flyout-close-button"
          onClick={() => void handleClose()}
        >
          ✕
        </button>
      </header>

      {!overview && <PulseLoading compact />}
      <section className="flyout-metrics">
        <div className="flyout-metric-card">
          <div className="flyout-metric-head">
            <span>CPU</span>
            <strong>{metric(overview?.cpuPercent, "%")}</strong>
          </div>
          <SparklineChart
            data={cpuHistory}
            color="#dda76d"
            height={22}
            ariaLabel="Gráfico de histórico de CPU no tray"
          />
        </div>

        <div className="flyout-metric-card">
          <div className="flyout-metric-head">
            <span>MEMÓRIA RAM</span>
            <strong>
              {metric(overview?.memoryUsedGb, " GB")} ({metric(memoryPercent, "%")})
            </strong>
          </div>
          <SparklineChart
            data={ramHistory}
            color="#8da9bf"
            height={22}
            ariaLabel="Gráfico de histórico de Memória RAM no tray"
          />
        </div>

        <div className={`flyout-metric-card disk-health-card disk-health-${diskHealth}`}>
          <div className="flyout-metric-head">
            <div className="disk-metric-label">
              <span>DISCO C:</span>
              <DiskHealthBadge health={diskHealth} />
            </div>
            <strong>
              {metric(overview?.diskUsedGb, " GB")} / {metric(overview?.diskTotalGb, " GB")}
            </strong>
          </div>
          <MiniCapacityBar
            usedGb={overview?.diskUsedGb}
            totalGb={overview?.diskTotalGb}
            percent={diskPercent}
            color={diskCapacityColor(diskHealth)}
            ariaLabel="Gráfico de alocação de disco no tray"
          />
        </div>
      </section>

      <section className="flyout-processes">
        <div className="flyout-section-title">
          <span>TOP PROCESSOS</span>
          <small>RAM</small>
        </div>
        <ul className="flyout-process-list">
          {(overview?.topProcesses?.length ? overview.topProcesses.slice(0, TOP_PROCESS_LIMIT).map((p, i) => (
            <li key={p.id}>
              <span className="flyout-process-rank">{String(i + 1).padStart(2, "0")}</span>
              <span className="flyout-process-name" title={[p.displayName || p.name, p.name, p.publisher].filter(Boolean).join(" · ")}>
                <strong>{p.displayName || p.name}</strong>
                <small>
                  {[p.name, p.publisher].filter(Boolean).join(" · ") || `PID ${p.id}`}
                  {(p.processCount ?? 1) > 1 ? ` · ${p.processCount} processos` : ""}
                </small>
              </span>
              <span className="flyout-process-mem">{metric(p.memoryMb, " MB")}</span>
              <button
                type="button"
                className="flyout-process-close"
                aria-label={`Fechar ${p.displayName || p.name}`}
                disabled={p.name.toLocaleLowerCase("pt-BR").startsWith("pulse")}
                onClick={() => { setProcessError(""); setProcessToClose(p); }}
              >
                ✕
              </button>
            </li>
          )) : <li className="muted">Processos indisponíveis nesta coleta.</li>)}
        </ul>
      </section>

      <footer className="flyout-actions">
        <button
          aria-label="Abrir interface completa"
          className="flyout-btn-primary"
          onClick={onOpenMain}
        >
          ABRIR INTERFACE COMPLETA ↗
        </button>
        <button
          aria-label="Sair do aplicativo"
          className="flyout-btn-danger"
          onClick={() => void handleExit()}
        >
          SAIR
        </button>
      </footer>
      {processToClose && (
        <div className="process-confirmation" role="dialog" aria-modal="true" aria-label="Confirmar encerramento do processo">
          <p>Fechar <strong>{processToClose.displayName || processToClose.name}</strong>?</p>
          <small>
            {isWslHostProcess(processToClose)
              ? "Isso desligará o WSL e encerrará todas as distribuições e processos Linux em execução."
              : `Serão encerrados todos os ${processToClose.processCount ?? 1} processos deste grupo${(processToClose.processCount ?? 1) === 1 ? "" : " ao mesmo tempo"}. Se necessário, será usado o encerramento forçado. Trabalho não salvo pode ser perdido.`}
          </small>
          {processError && <span role="status">{processError}</span>}
          <div>
            <button type="button" onClick={() => void confirmTerminateProcess()} disabled={terminatingId != null}>
              {terminatingId != null ? "FECHANDO…" : "FECHAR AGORA"}
            </button>
            <button type="button" onClick={() => setProcessToClose(null)} disabled={terminatingId != null}>CANCELAR</button>
          </div>
        </div>
      )}
    </aside>
  );
}

function App() {
  const isWindowForeground = useWindowForeground();
  const [windowLabel, setWindowLabel] = useState<string | null>(null);
  const [initialLoadAllowed, setInitialLoadAllowed] = useState(false);
  const [activeArea, setActiveArea] = useState("Visão geral");
  const [overview, setOverview] = useState<SystemOverview>();
  const [performance, setPerformance] = useState<Performance>();
  const [files, setFiles] = useState<RecentFile[]>([]);
  const [startupItems, setStartupItems] = useState<StartupItem[]>([]);
  const [startupItemsLoaded, setStartupItemsLoaded] = useState(false);
  const [startupEnabled, setStartupEnabled] = useState(false);
  const [fileQuery, setFileQuery] = useState("");
  const [fileCategory, setFileCategory] = useState("");
  const [fileSort, setFileSort] = useState("recent");
  const [busy, setBusy] = useState(false);
  const [booting, setBooting] = useState(true);
  const [overviewLoading, setOverviewLoading] = useState(true);
  const [filesLoading, setFilesLoading] = useState(true);
  const [toast, setToast] = useState<ToastStatus | null>({
    type: "loading",
    message: "Inicializando telemetria e arquivos recentes…",
  });
  const [error, setError] = useState("");

  const showToast = (
    type: "loading" | "success" | "info" | "error",
    message: string,
    autoDismissMs?: number,
  ) => {
    setToast({ type, message });
    if (autoDismissMs) {
      window.setTimeout(() => {
        setToast(current => (current?.message === message ? null : current));
      }, autoDismissMs);
    }
  };
  const [debugEnabled, setDebugEnabled] = useState(false);
  const [debugLog, setDebugLog] = useState<DebugEntry[]>([]);
  const [debugAuditInfo, setDebugAuditInfo] = useState<DebugAuditInfo>();
  const [debugQuery, setDebugQuery] = useState("");
  const [debugOutcome, setDebugOutcome] = useState<"all" | "ok" | "error">("all");
  const [debugCategory, setDebugCategory] = useState("all");
  const [debugLoading, setDebugLoading] = useState(false);
  const [showCloseDialog, setShowCloseDialog] = useState(false);
  const [storageSubTab, setStorageSubTab] = useState<"analyzer" | "recent" | "cleanup">("analyzer");
  const [customFolders, setCustomFolders] = useState<string[]>([]);
  const [newFolderPath, setNewFolderPath] = useState<string>("");
  const [folderActionLoading, setFolderActionLoading] = useState(false);
  const [cpuHistory, setCpuHistory] = useState<number[]>([]);
  const [ramHistory, setRamHistory] = useState<number[]>([]);
  const [startupSort, setStartupSort] = useState<"recommended" | "slowest" | "name" | "status">("recommended");
  const [startupSearch, setStartupSearch] = useState("");
  const [processToClose, setProcessToClose] = useState<TopProcess | null>(null);
  const [terminatingProcessId, setTerminatingProcessId] = useState<number | null>(null);
  const initialLoadStarted = useRef(false);
  const lastOverviewCollectionAt = useRef(0);

  const startupStats = useMemo(() => {
    const total = startupItems.length;
    const active = startupItems.filter(i => i.enabled).length;
    const disabled = startupItems.filter(i => !i.enabled).length;
    const slowest = startupItems.filter(
      i =>
        i.impact === "Alto impacto" ||
        (i.startupTimeMs != null && i.startupTimeMs >= 1000),
    ).length;
    return { total, active, disabled, slowest };
  }, [startupItems]);

  const filteredStartupItems = useMemo(() => {
    let list = [...startupItems];

    if (startupSearch.trim()) {
      const q = startupSearch.toLowerCase().trim();
      list = list.filter(
        i =>
          i.name.toLowerCase().includes(q) ||
          i.command.toLowerCase().includes(q) ||
          i.origin.toLowerCase().includes(q),
      );
    }

    list.sort((a, b) => {
      const compareSlowest = (first: StartupItem, second: StartupItem) => {
        const getWeight = (item: StartupItem) => {
          if (
            item.impact === "Alto impacto" ||
            (item.startupTimeMs != null && item.startupTimeMs >= 1000)
          )
            return 3;
          if (
            item.impact === "Médio impacto" ||
            (item.startupTimeMs != null && item.startupTimeMs >= 300)
          )
            return 2;
          if (item.impact === "Baixo impacto") return 1;
          return 0;
        };
        const diff = getWeight(second) - getWeight(first);
        if (diff !== 0) return diff;
        const timeA = first.startupTimeMs ?? 0;
        const timeB = second.startupTimeMs ?? 0;
        if (timeB !== timeA) return timeB - timeA;
        return first.name.localeCompare(second.name, "pt-BR");
      };

      if (startupSort === "recommended") {
        if (a.enabled !== b.enabled) return a.enabled ? -1 : 1;
        return compareSlowest(a, b);
      }
      if (startupSort === "slowest") {
        return compareSlowest(a, b);
      }
      if (startupSort === "name") {
        return a.name.localeCompare(b.name, "pt-BR");
      }
      if (startupSort === "status") {
        if (a.enabled !== b.enabled) return a.enabled ? -1 : 1;
        return a.name.localeCompare(b.name, "pt-BR");
      }
      return 0;
    });

    return list;
  }, [startupItems, startupSearch, startupSort]);

  const debugStats = useMemo(() => {
    const durations = debugLog
      .map(entry => entry.durationMs)
      .filter(value => typeof value === "number" && Number.isFinite(value));
    const errors = debugLog.filter(entry => entry.outcome === "error").length;
    const slowest = durations.length ? Math.max(...durations) : undefined;
    const average = durations.length
      ? Math.round(durations.reduce((total, value) => total + value, 0) / durations.length)
      : undefined;
    return { total: debugLog.length, errors, slowest, average };
  }, [debugLog]);

  const debugCategories = useMemo(
    () => Array.from(new Set(debugLog.map(entry => entry.category ?? "application"))).sort(),
    [debugLog],
  );

  const filteredDebugLog = useMemo(() => {
    const query = debugQuery.trim().toLocaleLowerCase("pt-BR");
    return [...debugLog]
      .reverse()
      .filter(entry => debugOutcome === "all" || entry.outcome === debugOutcome)
      .filter(entry => debugCategory === "all" || (entry.category ?? "application") === debugCategory)
      .filter(entry => {
        if (!query) return true;
        return [entry.operation, entry.message, entry.source, entry.category]
          .filter(Boolean)
          .some(value => value!.toLocaleLowerCase("pt-BR").includes(query));
      });
  }, [debugCategory, debugLog, debugOutcome, debugQuery]);


  useEffect(() => {
    invoke<string>("get_current_window_label")
      .then(label => {
        if (label) setWindowLabel(label);
      })
      .catch(() => setWindowLabel("main"));
  }, []);

  useEffect(() => {
    if (windowLabel === "main" && isWindowForeground === true) {
      setInitialLoadAllowed(true);
    }
  }, [isWindowForeground, windowLabel]);

  const memoryPercent =
    overview && typeof overview.memoryTotalGb === "number" && overview.memoryTotalGb > 0 && typeof overview.memoryUsedGb === "number"
      ? Math.round((overview.memoryUsedGb / overview.memoryTotalGb) * 100)
      : undefined;

  const diskPercent =
    overview && typeof overview.diskTotalGb === "number" && overview.diskTotalGb > 0 && typeof overview.diskUsedGb === "number"
      ? Math.round((overview.diskUsedGb / overview.diskTotalGb) * 100)
      : undefined;
  const diskHealth = getDiskCapacityHealth(diskPercent);
  const cpuSampleCount = cpuHistory.filter(value => Number.isFinite(value)).length;
  const ramSampleCount = ramHistory.filter(value => Number.isFinite(value)).length;
  // Uma única amostra renderiza só o marcador e parece um gráfico zerado.
  // Mantemos o skeleton até haver pontos suficientes para uma tendência legível.
  const cpuChartLoading = overviewLoading || (
    cpuSampleCount > 0 && cpuSampleCount < MIN_CHART_HISTORY_SAMPLES
  );
  const ramChartLoading = overviewLoading || (
    ramSampleCount > 0 && ramSampleCount < MIN_CHART_HISTORY_SAMPLES
  );

  const loadFiles = async (isInitial = false) => {
    setFilesLoading(true);
    if (!isInitial) {
      showToast("loading", "Buscando arquivos recentes…");
    }
    try {
      const result = await invoke<RecentFile[]>("list_recent_files", { limit: 50 });
      setFiles(Array.isArray(result) ? result : []);
      if (isInitial) {
        showToast("success", "Sistema e arquivos carregados com sucesso", 2500);
      } else {
        showToast("success", "Arquivos recentes atualizados", 2000);
      }
    } catch {
      setError("Arquivos recentes indisponíveis no momento.");
      showToast("error", "Não foi possível carregar os arquivos recentes", 3000);
    } finally {
      setFilesLoading(false);
    }
  };

  const loadCustomFolders = async () => {
    try {
      const result = await invoke<string[]>("get_custom_scan_folders");
      setCustomFolders(Array.isArray(result) ? result : []);
    } catch {
      // Continua seguro se falhar
    }
  };

  const handleAddCustomFolder = async (folderToAdd?: string) => {
    const target = (folderToAdd ?? newFolderPath).trim();
    if (!target) return;
    setFolderActionLoading(true);
    try {
      const updated = await invoke<string[]>("add_custom_scan_folder", { path: target });
      setCustomFolders(Array.isArray(updated) ? updated : []);
      setNewFolderPath("");
      showToast("success", `Pasta adicionada: ${target}`, 3000);
      void refresh();
    } catch (err) {
      showToast(
        "error",
        typeof err === "string" ? err : "Não foi possível adicionar a pasta.",
        4000,
      );
    } finally {
      setFolderActionLoading(false);
    }
  };

  const handleRemoveCustomFolder = async (pathToRemove: string) => {
    setFolderActionLoading(true);
    try {
      const updated = await invoke<string[]>("remove_custom_scan_folder", { path: pathToRemove });
      setCustomFolders(Array.isArray(updated) ? updated : []);
      showToast("info", `Pasta removida: ${pathToRemove}`, 3000);
      void refresh();
    } catch (err) {
      showToast(
        "error",
        typeof err === "string" ? err : "Não foi possível remover a pasta.",
        4000,
      );
    } finally {
      setFolderActionLoading(false);
    }
  };

  const loadPerformance = async () => {
    try {
      const result = await invoke<Performance>("get_performance_details");
      if (result && typeof result === "object") {
        setPerformance(result);
      }
    } catch {
      // Continua seguro se falhar
    }
  };

  const collect = async () => {
    lastOverviewCollectionAt.current = Date.now();
    try {
      const result = await invoke<SystemOverview>("get_system_overview");
      if (result && Array.isArray(result.topProcesses)) {
        setOverview(result);
        setError("");
        const cpu =
          typeof result.cpuPercent === "number" && !Number.isNaN(result.cpuPercent)
            ? Math.max(0, Math.min(100, result.cpuPercent))
            : Number.NaN;
        const ram =
          typeof result.memoryTotalGb === "number" && result.memoryTotalGb > 0 && typeof result.memoryUsedGb === "number"
            ? Math.round((result.memoryUsedGb / result.memoryTotalGb) * 100)
            : Number.NaN;
        setCpuHistory(prev =>
          [...prev, cpu].slice(-30),
        );
        setRamHistory(prev =>
          [...prev, ram].slice(-30),
        );
      } else {
        setOverview(undefined);
      }
    } catch {
      setError("Telemetria indisponível.");
    }
  };

  useEffect(() => {
    if (
      windowLabel !== "main" ||
      !initialLoadAllowed ||
      initialLoadStarted.current
    ) return;

    initialLoadStarted.current = true;

    // 1. Coleta inicial instantânea da máquina
    let disposed = false;
    let bootTimer: number | undefined;
    let chartSkeletonTimer: number | undefined;
    const minimumBootTime = new Promise<void>(resolve => {
      bootTimer = window.setTimeout(resolve, INITIAL_BOOT_MINIMUM_MS);
    });
    const minimumChartSkeletonTime = new Promise<void>(resolve => {
      chartSkeletonTimer = window.setTimeout(resolve, INITIAL_CHART_SKELETON_MS);
    });
    const initialCollection = collect();
    void Promise.allSettled([initialCollection, minimumChartSkeletonTime]).then(() => {
      if (!disposed) setOverviewLoading(false);
    });
    void Promise.allSettled([initialCollection, minimumBootTime]).then(() => {
      if (!disposed) setBooting(false);
    });

    // 2. Registro e itens de boot escalonados para não concorrer na largada
    const t1 = window.setTimeout(() => {
      void invoke<StartupRegistration>("get_startup_registration")
        .then(x => setStartupEnabled(x?.enabled === true))
        .catch(() => undefined);
      void loadStartupItems();
    }, 150);

    // 3. Busca de arquivos recentes com indicador transparente e sem travar o mouse
    const t2 = window.setTimeout(() => {
      void loadFiles(true);
    }, 300);

    // 4. Detalhes de GPU / Desempenho escalonados
    const t3 = window.setTimeout(() => {
      void loadPerformance();
    }, 550);

    return () => {
      disposed = true;
      if (bootTimer !== undefined) window.clearTimeout(bootTimer);
      if (chartSkeletonTimer !== undefined) window.clearTimeout(chartSkeletonTimer);
      window.clearTimeout(t1);
      window.clearTimeout(t2);
      window.clearTimeout(t3);
    };
  }, [initialLoadAllowed, windowLabel]);

  useEffect(() => {
    if (windowLabel !== "main" || isWindowForeground !== true) return;

    // Retoma imediatamente ao voltar ao primeiro plano, sem duplicar a coleta da abertura.
    if (Date.now() - lastOverviewCollectionAt.current > 500) {
      void collect();
      void loadPerformance();
    }

    const delayUntilNextSample = Math.max(
      0,
      Math.min(
        INITIAL_CHART_SAMPLE_DELAY_MS,
        TELEMETRY_POLL_INTERVAL_MS - (Date.now() - lastOverviewCollectionAt.current),
      ),
    );
    let telemetryTimer: number | undefined;
    const startTimer = window.setTimeout(() => {
      void collect();
      telemetryTimer = window.setInterval(
        () => void collect(),
        TELEMETRY_POLL_INTERVAL_MS,
      );
    }, delayUntilNextSample);

    return () => {
      window.clearTimeout(startTimer);
      if (telemetryTimer !== undefined) window.clearInterval(telemetryTimer);
    };
  }, [isWindowForeground, windowLabel]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("pulse://close-requested", () => {
      setShowCloseDialog(true);
    })
      .then(cleanup => {
        unlisten = cleanup;
      })
      .catch(() => undefined);

    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  const loadDebugAudit = async () => {
    setDebugLoading(true);
    try {
      const [enabled, entries, info] = await Promise.all([
        invoke<boolean>("get_debug_mode"),
        invoke<DebugEntry[]>("get_debug_log"),
        invoke<DebugAuditInfo>("get_debug_audit_info"),
      ]);
      setDebugEnabled(enabled === true);
      setDebugLog(Array.isArray(entries) ? entries : []);
      if (info && typeof info === "object") setDebugAuditInfo(info);
    } catch {
      setError("Não foi possível ler a trilha de auditoria local.");
    } finally {
      setDebugLoading(false);
    }
  };

  useEffect(() => {
    if (activeArea === "Debug") void loadDebugAudit();
  }, [activeArea]);

  useEffect(() => {
    invoke<boolean>("get_debug_mode")
      .then(setDebugEnabled)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    if (activeArea === "Armazenamento") {
      void loadFiles();
      void loadCustomFolders();
    }
    if (activeArea === "Desempenho") {
      void loadPerformance();
    }
    if (activeArea === "Inicialização e segurança") {
      void loadStartupItems();
    }
  }, [activeArea]);

  const loadStartupItems = async () => {
    try {
      const result = await invoke<StartupItem[]>("list_startup_items");
      const items = Array.isArray(result) ? result : [];
      setStartupItems(items);
      setStartupItemsLoaded(true);
      setOverview(previous =>
        previous ? { ...previous, startupItemCount: items.length } : previous,
      );
    } catch {
      setStartupItems([]);
      setStartupItemsLoaded(false);
    }
  };

  const handleToggleStartup = (item: StartupItem, nextChecked: boolean) => {
    void invoke<StartupItem>("set_startup_item_enabled", {
      name: item.name,
      origin: item.origin,
      enabled: nextChecked,
    })
      .then(res => {
        setStartupItems(xs =>
          xs.map(x =>
            x.name.toLowerCase() === item.name.toLowerCase()
              ? { ...x, enabled: res?.enabled ?? nextChecked }
              : x,
          ),
        );
      })
      .catch(() => {
        setError("Não foi possível alterar este item de inicialização.");
      });
  };

  const refresh = async () => {
    setBusy(true);
    setError("");
    showToast("loading", "Atualizando dados da máquina…");
    try {
      const result = await invoke<RecentFile[]>("refresh_recent_files");
      setFiles(Array.isArray(result) ? result : []);
      await Promise.allSettled([
        collect(),
        loadPerformance(),
        loadStartupItems(),
        loadCustomFolders(),
      ]);
      showToast("success", "Dados atualizados com sucesso", 2500);
    } catch {
      setError("Não foi possível atualizar os dados.");
      showToast("error", "Não foi possível atualizar os dados", 3000);
    } finally {
      setBusy(false);
    }
  };

  const togglePulse = async (enabled: boolean) => {
    setStartupEnabled(enabled);
    try {
      const result = await invoke<StartupRegistration>("set_startup_registration", { enabled });
      setStartupEnabled(result.enabled);
    } catch {
      setStartupEnabled(!enabled);
      setError("O Windows não permitiu alterar a inicialização do Pulse.");
    }
  };

  const toggleDebug = async (enabled: boolean) => {
    setDebugEnabled(enabled);
    try {
      await invoke<boolean>("set_debug_mode", { enabled });
      if (activeArea === "Debug") await loadDebugAudit();
    } catch {
      setDebugEnabled(!enabled);
      setError("Não foi possível alterar o modo Debug.");
    }
  };

  const clearDebugAudit = async () => {
    try {
      await invoke("clear_debug_log");
      setDebugLog([]);
      setDebugAuditInfo(info => info ? { ...info, entryCount: 0 } : info);
      showToast("success", "Histórico desta execução removido", 2200);
    } catch {
      showToast("error", "Não foi possível limpar o histórico", 3000);
    }
  };

  const copyDebugAudit = async () => {
    const report = {
      generatedAt: new Date().toISOString(),
      audit: debugAuditInfo,
      filters: { query: debugQuery || null, outcome: debugOutcome, category: debugCategory },
      entries: filteredDebugLog,
    };
    try {
      await navigator.clipboard.writeText(JSON.stringify(report, null, 2));
      showToast("success", "Relatório JSON copiado", 2200);
    } catch {
      showToast("error", "O Windows não permitiu copiar o relatório", 3000);
    }
  };

  const exportDebugAudit = () => {
    const report = {
      generatedAt: new Date().toISOString(),
      audit: debugAuditInfo,
      entries: debugLog,
    };
    const blob = new Blob([JSON.stringify(report, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `pulse-auditoria-${new Date().toISOString().replace(/[:.]/g, "-")}.json`;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const handleMinimizeToTray = async (remember = false) => {
    setShowCloseDialog(false);
    try {
      await invoke(remember ? "minimize_to_tray_and_remember" : "minimize_to_tray");
    } catch {
      setError("Não foi possível minimizar para o tray icon.");
    }
  };

  const handleExitApp = async () => {
    setShowCloseDialog(false);
    try {
      await invoke("exit_app");
    } catch {
      window.close();
    }
  };

  const handleCancelClose = () => {
    setShowCloseDialog(false);
  };

  const handleTerminateProcess = async () => {
    if (!processToClose) return;
    setTerminatingProcessId(processToClose.id);
    try {
      await invoke("terminate_process", { id: processToClose.id });
      setOverview(previous => previous ? {
        ...previous,
        topProcesses: previous.topProcesses.filter(process => process.id !== processToClose.id),
      } : previous);
      showToast("success", `${processToClose.displayName || processToClose.name} foi fechado`, 2500);
      setProcessToClose(null);
      window.setTimeout(() => void collect(), 500);
    } catch (error) {
      showToast("error", typeof error === "string" ? error : "O Windows não permitiu fechar este aplicativo.", 4000);
    } finally {
      setTerminatingProcessId(null);
    }
  };

  const handleOpenMainFromFlyout = async () => {
    try {
      await invoke("show_main_window");
    } catch {
      // Ignora erro
    }
  };

  const filteredFiles = useMemo(() => {
    const query = fileQuery.trim().toLocaleLowerCase("pt-BR");
    const list = files.filter(
      file =>
        (!fileCategory || file.category === fileCategory) &&
        (!query || `${file.name} ${file.path}`.toLocaleLowerCase("pt-BR").includes(query)),
    );

    return list.sort((a, b) => {
      switch (fileSort) {
        case "oldest":
          return a.modifiedAtUnixMs - b.modifiedAtUnixMs;
        case "name":
          return a.name.localeCompare(b.name, "pt-BR");
        case "size":
          return b.sizeBytes - a.sizeBytes;
        case "recent":
        default:
          return b.modifiedAtUnixMs - a.modifiedAtUnixMs;
      }
    });
  }, [files, fileCategory, fileQuery, fileSort]);

  const openFile = (file: RecentFile, location = false) =>
    void invoke(location ? "open_recent_file_location" : "open_recent_file", {
      path: file.path,
    }).catch(() => setError("Não foi possível abrir esse item."));

  // Se esta janela for o painel flutuante do tray icon, renderiza a barra flutuante
  if (windowLabel === "tray-flyout") {
    return (
      <TrayFlyout
        isForeground={isWindowForeground}
        onOpenMain={() => void handleOpenMainFromFlyout()}
      />
    );
  }

  const processes = overview?.topProcesses ?? [];
  const module = copy[activeArea] || copy["Visão geral"];
  const rawVramBytes = performance?.vramBytes?.value;
  const rawGpuTemperature = performance?.temperatureCelsius?.value;
  const vramBytes =
    typeof rawVramBytes === "number" && Number.isFinite(rawVramBytes)
      ? rawVramBytes
      : undefined;
  const gpuTemperature =
    typeof rawGpuTemperature === "number" && Number.isFinite(rawGpuTemperature)
      ? rawGpuTemperature
      : undefined;
  const hasOptionalGpuMetrics = vramBytes !== undefined || gpuTemperature !== undefined;

  return (
    <CrashBoundary key={activeArea} onRecover={() => setActiveArea("Visão geral")}>
      <main className="app-shell">
        {booting && <div className="boot-overlay"><PulseLoading /><button onClick={() => setBooting(false)}>Continuar para o painel</button></div>}
        <aside className="sidebar">
          <div className="brand">
            <PulseMark />
            <div>
              <h1>Pulse</h1>
              <p>Windows observability</p>
            </div>
          </div>
          <nav aria-label="Navegação principal">
            <p className="nav-label">MÓDULOS</p>
            <ul>
              {navigationItems.map((item, i) => (
                <li key={item}>
                  <button
                    aria-current={activeArea === item ? "page" : undefined}
                    className={`nav-item ${activeArea === item ? "active" : ""}`}
                    onClick={() => setActiveArea(item)}
                  >
                    <ModuleIcon index={i} />
                    <span className="nav-title">{item}</span>
                  </button>
                </li>
              ))}
            </ul>
          </nav>
          <div className="sidebar-foot">
            <button
              type="button"
              className={`about-nav ${activeArea === "Informações" ? "active" : ""}`}
              aria-current={activeArea === "Informações" ? "page" : undefined}
              onClick={() => setActiveArea("Informações")}
            >
              <span aria-hidden="true">i</span>
              Sobre o Pulse
            </button>
            <div className="sidebar-status">
              <span className="connection-state">LOCAL</span>
              <p>
                Sem nuvem
                <br />
                Sem conta
              </p>
            </div>
          </div>
        </aside>

        <section className="content">
          <header className="command-bar">
            <div>
              <span>HOST</span>
              <b>ESTAÇÃO LOCAL</b>
            </div>
            <div>
              <span>COLETA</span>
              <b className={overview ? "state-good" : "state-pending"}>
                {overview ? "ATIVA" : "INICIANDO"}
              </b>
            </div>
            <label className="header-toggle">
              <span>INICIAR COM WINDOWS</span>
              <input
                aria-label="Iniciar com o Windows"
                type="checkbox"
                checked={startupEnabled}
                onChange={e => void togglePulse(e.target.checked)}
              />
            </label>
            <label className="header-toggle">
              <span>DEBUG</span>
              <input
                aria-label="Modo Debug"
                type="checkbox"
                checked={debugEnabled}
                onChange={e => void toggleDebug(e.target.checked)}
              />
            </label>
            <button
              aria-label="Minimizar para o tray icon"
              className="tray-minimize-button"
              title="Minimizar para a área de notificação da barra de tarefas"
              onClick={() => void handleMinimizeToTray()}
            >
              BANDEJA 🗕
            </button>
            <button className="refresh-button" onClick={() => void refresh()}>
              RELER DADOS <span>↻</span>
            </button>
          </header>

          <div className={`overview-heading${activeArea === "Armazenamento" ? " storage-overview-heading" : ""}`}>
            <div>
              <p className="eyebrow">
                {module.eyebrow} / {activeArea === "Informações" ? "INFO" : `0${navigationItems.indexOf(activeArea) + 1}`}
              </p>
              <h2>{module.title}</h2>
            </div>
            <p className="intro">{module.intro}</p>
          </div>

          <div className="workspace">
            {activeArea === "Visão geral" && (
              <section className="dashboard">
                <h3 className="sr-only">Painel de telemetria local</h3>
                <div className="metric-grid">
                  <span className="sr-only">Inicialização e cobertura</span>
                  <button
                    aria-label="Abrir desempenho — CPU"
                    className="metric-card"
                    onClick={() => setActiveArea("Desempenho")}
                  >
                    <div className="metric-card-top">
                      <p>CPU agora</p>
                      <span className="metric-badge-live">AO VIVO</span>
                    </div>
                    <strong>{metric(overview?.cpuPercent, "%")}</strong>
                    <SparklineChart
                      data={cpuHistory}
                      loading={cpuChartLoading}
                      color="#dda76d"
                      height={32}
                      showGrid
                      ariaLabel="Gráfico de tendência de CPU"
                    />
                  </button>
                  <button
                    className="metric-card"
                    onClick={() => setActiveArea("Desempenho")}
                  >
                    <div className="metric-card-top">
                      <p>Memória RAM</p>
                      <span className="metric-badge-live">AO VIVO</span>
                    </div>
                    <strong>{metric(overview?.memoryUsedGb, " GB")}</strong>
                    <SparklineChart
                      data={ramHistory}
                      loading={ramChartLoading}
                      color="#8da9bf"
                      height={32}
                      showGrid
                      ariaLabel="Gráfico de tendência de memória RAM"
                    />
                  </button>
                  <button
                    className={`metric-card disk-health-card disk-health-${diskHealth}`}
                    aria-label={`Abrir armazenamento — ${diskHealth === "critical" ? "espaço crítico" : diskHealth === "warning" ? "armazenamento quase cheio" : "capacidade local"}`}
                    onClick={() => setActiveArea("Armazenamento")}
                  >
                    <div className="metric-card-top">
                      <p>Disco C:</p>
                      <DiskHealthBadge health={diskHealth} />
                    </div>
                    <strong>{metric(overview?.diskUsedGb, " GB")}</strong>
                    <MiniCapacityBar
                      usedGb={overview?.diskUsedGb}
                      totalGb={overview?.diskTotalGb}
                      percent={diskPercent}
                      color={diskCapacityColor(diskHealth)}
                      ariaLabel="Gráfico de ocupação de disco"
                      loading={overviewLoading}
                    />
                  </button>
                  <button
                    aria-label="Abrir inicialização e segurança"
                    className="metric-card"
                    onClick={() => setActiveArea("Inicialização e segurança")}
                  >
                    <div className="metric-card-top">
                      <p>Inicialização</p>
                      <span className="startup-active-badge">
                        {startupItemsLoaded
                          ? `${startupStats.active} ${startupStats.active === 1 ? "ATIVO" : "ATIVOS"}`
                          : "ATIVOS —"}
                      </span>
                    </div>
                    <strong>{overview?.startupItemCount ?? "—"}</strong>
                    <div className="startup-metric-visual" aria-hidden="true">
                      <div className="startup-dots">
                        {Array.from({ length: Math.min(8, overview?.startupItemCount ?? 0) }).map(
                          (_, i) => (
                            <span key={i} className="startup-dot active" />
                          ),
                        )}
                      </div>
                    </div>
                    <small>Itens detectados no Windows</small>
                  </button>
                </div>

                <div className="detail-grid">
                  <section className="panel process-panel">
                    <div className="panel-title">
                      <h3>Processos com mais RAM</h3>
                      <span className="live-dot">AO VIVO</span>
                    </div>
                    <ProcessList processes={processes} />
                  </section>
                  <FileList
                    files={filteredFiles}
                    busy={busy || filesLoading}
                    fileQuery={fileQuery}
                    fileCategory={fileCategory}
                    fileSort={fileSort}
                    onQueryChange={setFileQuery}
                    onCategoryChange={setFileCategory}
                    onSortChange={setFileSort}
                    onRefresh={() => void refresh()}
                    onOpen={openFile}
                  />
                </div>
              </section>
            )}

            {activeArea === "Atividade" && (
              <section className="module-dashboard">
                <section className="panel tall-panel">
                  <div className="panel-title">
                    <h3>Até 25 grupos de aplicativos com maior consumo de memória</h3>
                    <span className="live-dot">AO VIVO</span>
                  </div>
                  <ProcessList
                    processes={processes}
                    onTerminate={setProcessToClose}
                    terminatingId={terminatingProcessId}
                  />
                </section>
              </section>
            )}

            {activeArea === "Desempenho" && (
              <section className="module-dashboard">
                <div className={`focused-grid ${hasOptionalGpuMetrics ? "" : "without-optional-gpu-metrics"}`}>
                  <section className="panel focused-card">
                    <p>CPU AGORA</p>
                    <strong>{metric(overview?.cpuPercent, "%")}</strong>
                    <DetailedResourceChart
                      title="USO DA CPU EM TEMPO REAL"
                      history={cpuHistory}
                      currentValue={metric(overview?.cpuPercent, "%")}
                      color="#dda76d"
                      ariaLabel="Gráfico detalhado de CPU"
                      loading={cpuChartLoading}
                    />
                    <small>Carga média de processamento</small>
                  </section>
                  <section className="panel focused-card">
                    <p>MEMÓRIA RAM</p>
                    <strong>
                      {metric(overview?.memoryUsedGb, " GB")} /{" "}
                      {metric(overview?.memoryTotalGb, " GB")}
                    </strong>
                    <DetailedResourceChart
                      title="ALOCAÇÃO DE MEMÓRIA RAM"
                      history={ramHistory}
                      currentValue={memoryPercent !== undefined ? `${memoryPercent}%` : "—"}
                      subValue={
                        overview
                          ? `${metric(overview.memoryUsedGb, " GB")} em uso de ${metric(overview.memoryTotalGb, " GB")}`
                          : undefined
                      }
                      color="#8da9bf"
                      ariaLabel="Gráfico detalhado de Memória RAM"
                      loading={ramChartLoading}
                    />
                    <small>
                      {memoryPercent !== undefined
                        ? `${memoryPercent}% em utilização`
                        : "Telemetria de memória"}
                    </small>
                  </section>
                  <section className="panel focused-card">
                    <p>GPU</p>
                    <strong>{performance?.gpu?.value || "Indisponível"}</strong>
                    <HardwareGaugeBar
                      label="CONTROLADORA GRÁFICA"
                      valueText={performance?.gpu?.value ? "Detectada" : "Indisponível"}
                      percent={null}
                      statusText="Utilização não exposta por esta fonte"
                      type="gpu"
                    />
                    <small>
                      {performance?.gpu?.unavailableReason || "Controladora gráfica do sistema"}
                    </small>
                  </section>
                  {hasOptionalGpuMetrics && (
                    <section className="panel focused-card">
                      <p>
                        {vramBytes !== undefined && gpuTemperature !== undefined
                          ? "VRAM / TEMPERATURA"
                          : vramBytes !== undefined
                            ? "VRAM DEDICADA"
                            : "TEMPERATURA GPU"}
                      </p>
                      <strong>
                        {vramBytes !== undefined
                          ? metric(vramBytes / (1024 * 1024), " MB")
                          : `${metric(gpuTemperature)} °C`}
                      </strong>
                      <div className="focused-card-hw-split">
                        {vramBytes !== undefined && (
                          <HardwareGaugeBar
                            label="VRAM DEDICADA"
                            valueText={metric(vramBytes / (1024 * 1024), " MB")}
                            percent={null}
                            statusText="Memória dedicada"
                            type="vram"
                          />
                        )}
                        {gpuTemperature !== undefined && (
                          <HardwareGaugeBar
                            label="TEMPERATURA"
                            valueText={`${metric(gpuTemperature)} °C`}
                            percent={Math.min(100, Math.max(0, gpuTemperature))}
                            statusText="Sensor ativo"
                            type="temperature"
                          />
                        )}
                      </div>
                      <small>
                        {vramBytes !== undefined && gpuTemperature !== undefined
                          ? "Memória dedicada e sensor térmico ativos"
                          : vramBytes !== undefined
                            ? "Memória dedicada reportada pelo driver"
                            : "Sensor térmico ativo"}
                      </small>
                    </section>
                  )}
                </div>
              </section>
            )}

            {activeArea === "Armazenamento" && (
              <section className="module-dashboard storage-module">
                <nav className="storage-tabs-bar" aria-label="Ferramentas de armazenamento">
                  <button
                    type="button"
                    className={`storage-tab-btn ${storageSubTab === "analyzer" ? "active" : ""}`}
                    onClick={() => setStorageSubTab("analyzer")}
                  >
                    <span>ANÁLISE DE DISCO (TREE SIZE)</span>
                    <span className="storage-tab-badge">NOVO</span>
                  </button>
                  <button
                    type="button"
                    className={`storage-tab-btn ${storageSubTab === "recent" ? "active" : ""}`}
                    onClick={() => setStorageSubTab("recent")}
                  >
                    <span>ARQUIVOS RECENTES</span>
                  </button>
                  <button
                    type="button"
                    className={`storage-tab-btn ${storageSubTab === "cleanup" ? "active" : ""}`}
                    onClick={() => setStorageSubTab("cleanup")}
                  >
                    <span>LIMPEZA DE DISCO</span>
                    <span className="storage-tab-badge">SEGURO</span>
                  </button>
                </nav>

                <div
                  style={{
                    display: storageSubTab === "analyzer" ? "flex" : "none",
                    flexDirection: "column",
                    flex: 1,
                    minHeight: 0,
                    height: "100%",
                  }}
                >
                  <DiskAnalyzer
                    overviewDiskUsedGb={overview?.diskUsedGb}
                    overviewDiskTotalGb={overview?.diskTotalGb}
                  />
                </div>

                <div
                  style={{
                    display: storageSubTab === "recent" ? "flex" : "none",
                    flexDirection: "column",
                    flex: 1,
                    minHeight: 0,
                    height: "100%",
                  }}
                >
                  <ScanFoldersBar
                    customFolders={customFolders}
                    newFolderPath={newFolderPath}
                    loading={folderActionLoading}
                    onNewFolderPathChange={setNewFolderPath}
                    onAddFolder={handleAddCustomFolder}
                    onRemoveFolder={handleRemoveCustomFolder}
                  />
                  <FileList
                    files={filteredFiles}
                    busy={busy || filesLoading}
                    fileQuery={fileQuery}
                    fileCategory={fileCategory}
                    fileSort={fileSort}
                    onQueryChange={setFileQuery}
                    onCategoryChange={setFileCategory}
                    onSortChange={setFileSort}
                    onRefresh={() => void refresh()}
                    onOpen={openFile}
                  />
                </div>

                <div
                  style={{
                    display: storageSubTab === "cleanup" ? "flex" : "none",
                    flexDirection: "column",
                    flex: 1,
                    minHeight: 0,
                    height: "100%",
                  }}
                >
                  <DiskCleanup />
                </div>
              </section>
            )}

            {activeArea === "Inicialização e segurança" && (
              <section className="module-dashboard">
                <div className="startup-stats-row">
                  <div className="startup-stat-card">
                    <span className="stat-label">Total de Itens</span>
                    <strong className="stat-value">{startupStats.total}</strong>
                    <small className="stat-sub">Registrados no boot</small>
                  </div>
                  <div className="startup-stat-card">
                    <span className="stat-label">Ativados</span>
                    <strong className="stat-value stat-active">{startupStats.active}</strong>
                    <small className="stat-sub">Iniciam com o Windows</small>
                  </div>
                  <div className="startup-stat-card">
                    <span className="stat-label">Desativados</span>
                    <strong className="stat-value stat-disabled">{startupStats.disabled}</strong>
                    <small className="stat-sub">Ignorados no boot</small>
                  </div>
                  <div className={`startup-stat-card ${startupStats.slowest > 0 ? "stat-card-warning" : ""}`}>
                    <span className="stat-label">Mais Lentos</span>
                    <strong className="stat-value stat-slow">{startupStats.slowest}</strong>
                    <small className="stat-sub">Alto impacto no boot</small>
                  </div>
                </div>

                {startupStats.slowest > 0 && (
                  <div className="startup-alert" role="note">
                    <span className="startup-alert-badge">AVISO DE DESEMPENHO</span>
                    <p>
                      <strong>{startupStats.slowest} {startupStats.slowest === 1 ? "aplicativo de alto impacto detectado" : "aplicativos de alto impacto detectados"}.</strong> Eles demandam mais tempo de CPU e disco na inicialização. Desativar itens desnecessários acelera o boot do Windows.
                    </p>
                  </div>
                )}

                <section className="panel startup-card">
                  <div className="panel-title">
                    <h3>Itens de Inicialização</h3>
                    <small className="source-note">Registro HKCU/HKLM e StartupApproved</small>
                  </div>

                  <div className="startup-tools">
                    <input
                      type="search"
                      placeholder="Buscar por nome ou comando..."
                      value={startupSearch}
                      onChange={e => setStartupSearch(e.target.value)}
                      aria-label="Buscar itens de inicialização"
                    />
                    <select
                      className="sort-select"
                      aria-label="Ordenar inicialização"
                      value={startupSort}
                      onChange={e => setStartupSort(e.target.value as "recommended" | "slowest" | "name" | "status")}
                    >
                      <option value="recommended">Ativos e mais lentos primeiro</option>
                      <option value="slowest">Mais lentos primeiro</option>
                      <option value="name">Nome (A-Z)</option>
                      <option value="status">Ativados primeiro</option>
                    </select>
                  </div>

                  <div className="startup-list">
                    {filteredStartupItems.length ? (
                      filteredStartupItems.map(item => {
                        const isHigh =
                          item.impact === "Alto impacto" ||
                          (item.startupTimeMs != null && item.startupTimeMs >= 1000);
                        const isMed =
                          item.impact === "Médio impacto" ||
                          (item.startupTimeMs != null &&
                            item.startupTimeMs >= 300 &&
                            item.startupTimeMs < 1000);
                        const impactClass = isHigh
                          ? "impact-high"
                          : isMed
                            ? "impact-medium"
                            : "impact-low";
                        const durationStr = formatStartupDuration(item.startupTimeMs);

                        return (
                          <div
                            className={`startup-item ${!item.enabled ? "item-disabled" : ""} ${isHigh ? "item-high-impact" : ""}`}
                            key={`${item.origin}-${item.name}`}
                          >
                            <div className="startup-item-content">
                              <div className="startup-item-header">
                                <strong className="startup-name">{item.name}</strong>
                                <span className={`impact-badge ${impactClass}`}>
                                  {item.impact || "Não medido"}
                                  {durationStr ? ` · ${durationStr}` : ""}
                                </span>
                                <span
                                  className={`status-badge ${item.enabled ? "status-enabled" : "status-disabled"}`}
                                >
                                  {item.enabled ? "Ativado" : "Desativado"}
                                </span>
                              </div>
                              <small title={item.command}>
                                {item.origin} · {item.command}
                              </small>
                            </div>
                            {item.canChange ? (
                              <div className="startup-toggle-cell">
                                <input
                                  aria-label={`${item.enabled ? "Desativar" : "Ativar"} ${item.name}`}
                                  type="checkbox"
                                  checked={item.enabled}
                                  onChange={e => handleToggleStartup(item, e.target.checked)}
                                />
                              </div>
                            ) : (
                              <small className="startup-readonly">
                                {item.unavailableReason ?? "Somente leitura"}
                              </small>
                            )}
                          </div>
                        );
                      })
                    ) : (
                      <p className="muted">Nenhum item de inicialização encontrado.</p>
                    )}
                  </div>
                </section>
              </section>
            )}

            {activeArea === "Debug" && (
              <section className="module-dashboard debug-dashboard">
                <section className="panel debug-overview">
                  <div className="debug-overview-heading">
                    <div className="debug-control-copy">
                      <div className={`debug-live-indicator ${debugEnabled ? "is-active" : ""}`}>
                        <span aria-hidden="true" />
                        {debugEnabled ? "AUDITORIA ATIVA" : "AUDITORIA PAUSADA"}
                      </div>
                      <h3>Registro técnico da execução</h3>
                      <p>
                        Operações instrumentadas, duração e resultado — sem inspecionar arquivos ou enviar dados.
                      </p>
                    </div>
                    <label className="debug-switch">
                      <span>{debugEnabled ? "Registrando" : "Pausado"}</span>
                      <input
                        aria-label="Modo Debug"
                        type="checkbox"
                        checked={debugEnabled}
                        onChange={e => void toggleDebug(e.target.checked)}
                      />
                    </label>
                  </div>

                  <section className="debug-stats" aria-label="Resumo da auditoria">
                    <article className="debug-stat"><span>EVENTOS</span><strong>{debugStats.total}</strong><small>nesta execução</small></article>
                    <article className={`debug-stat ${debugStats.errors ? "has-error" : ""}`}><span>FALHAS</span><strong>{debugStats.errors}</strong><small>resultado técnico</small></article>
                    <article className="debug-stat"><span>TEMPO MÉDIO</span><strong>{debugStats.average != null ? `${debugStats.average} ms` : "—"}</strong><small>operações medidas</small></article>
                    <article className="debug-stat"><span>MAIS LENTA</span><strong>{debugStats.slowest != null ? `${debugStats.slowest} ms` : "—"}</strong><small>nesta execução</small></article>
                  </section>

                  <section className="debug-transparency" aria-label="Escopo e privacidade da auditoria">
                    <div><span>VERSÃO / PLATAFORMA</span><strong>{debugAuditInfo ? `${debugAuditInfo.applicationVersion} · ${debugAuditInfo.platform}` : "Consultando…"}</strong></div>
                    <div><span>RETENÇÃO</span><strong>{debugAuditInfo ? `${debugAuditInfo.entryCount} de ${debugAuditInfo.retentionLimit} eventos` : "Consultando…"}</strong></div>
                    <div><span>PERSISTÊNCIA REAL</span><strong>{debugAuditInfo?.persistence ?? "Somente a execução atual"}</strong></div>
                    <div><span>RESIDÊNCIA DOS DADOS</span><strong>{debugAuditInfo?.dataResidency ?? "Local neste computador"}</strong></div>
                  </section>
                </section>

                <section className="panel debug-events-panel">
                  <div className="debug-toolbar">
                    <div>
                      <p className="eyebrow">TRILHA DE EVENTOS</p>
                      <h3>{filteredDebugLog.length} {filteredDebugLog.length === 1 ? "evento visível" : "eventos visíveis"}</h3>
                    </div>
                    <div className="debug-actions">
                      <button type="button" onClick={() => void loadDebugAudit()} disabled={debugLoading}>{debugLoading ? "LENDO…" : "ATUALIZAR"}</button>
                      <button type="button" onClick={() => void copyDebugAudit()} disabled={!debugLog.length}>COPIAR JSON</button>
                      <button type="button" onClick={exportDebugAudit} disabled={!debugLog.length}>EXPORTAR</button>
                      <button type="button" className="danger" onClick={() => void clearDebugAudit()} disabled={!debugLog.length}>LIMPAR</button>
                    </div>
                  </div>
                  <div className="debug-filters">
                    <input aria-label="Buscar na auditoria" value={debugQuery} onChange={e => setDebugQuery(e.target.value)} placeholder="Buscar operação, mensagem ou origem…" />
                    <select aria-label="Filtrar resultado" value={debugOutcome} onChange={e => setDebugOutcome(e.target.value as "all" | "ok" | "error")}>
                      <option value="all">Todos os resultados</option>
                      <option value="ok">Somente concluídos</option>
                      <option value="error">Somente falhas</option>
                    </select>
                    <select aria-label="Filtrar categoria" value={debugCategory} onChange={e => setDebugCategory(e.target.value)}>
                      <option value="all">Todas as categorias</option>
                      {debugCategories.map(category => <option key={category} value={category}>{debugCategoryLabel(category)}</option>)}
                    </select>
                  </div>
                  <div className="debug-event-list" aria-live="polite">
                    {filteredDebugLog.length ? filteredDebugLog.map((entry, index) => (
                      <article className={`debug-event ${entry.outcome === "error" ? "is-error" : ""}`} key={`${entry.sequence ?? index}-${entry.timestampUnixMs}`}>
                        <div className="debug-event-marker" aria-hidden="true" />
                        <div className="debug-event-main">
                          <div className="debug-event-heading">
                            <span className={`debug-outcome ${entry.outcome === "error" ? "is-error" : ""}`}>{entry.outcome === "error" ? "FALHA" : "OK"}</span>
                            <strong>{debugOperationLabel(entry.operation)}</strong>
                            <code>{entry.operation}</code>
                          </div>
                          <p>{entry.message}</p>
                          <div className="debug-event-meta">
                            <span>{new Date(entry.timestampUnixMs).toLocaleString("pt-BR")}</span>
                            <span>{debugCategoryLabel(entry.category)}</span>
                            <span>{entry.source ?? "rust-backend"}</span>
                            {entry.sequence != null && <span>#{entry.sequence}</span>}
                          </div>
                        </div>
                        <strong className="debug-duration">{entry.durationMs} ms</strong>
                      </article>
                    )) : (
                      <div className="debug-empty">
                        <strong>{debugLog.length ? "Nenhum evento corresponde aos filtros." : "Nenhum evento registrado nesta execução."}</strong>
                        <span>{debugEnabled ? "Use o Pulse normalmente e atualize esta trilha para ver as operações instrumentadas." : "Ative a auditoria para registrar as próximas operações."}</span>
                      </div>
                    )}
                  </div>
                </section>
              </section>
            )}

            {activeArea === "Informações" && (
              <section className="module-dashboard info-page" aria-labelledby="about-pulse-title">
                <article className="panel info-card">
                  <div className="info-identity">
                    <PulseMark />
                    <div>
                      <p className="eyebrow">PULSE / WINDOWS OBSERVABILITY</p>
                      <h3 id="about-pulse-title">Clareza local sobre o seu sistema.</h3>
                    </div>
                  </div>
                  <p className="info-summary">
                    Uma central Windows-first feita para observar, explicar e organizar o sistema
                    sem enviar telemetria para a nuvem.
                  </p>
                  <dl className="info-details">
                    <div>
                      <dt>Autor</dt>
                      <dd>Lucas Lagrimante</dd>
                    </div>
                    <div>
                      <dt>Site</dt>
                      <dd>
                        <button type="button" onClick={() => void openUrl("https://lucaslagrimante.github.io/")}>
                          lucaslagrimante.github.io
                          <span aria-hidden="true">↗</span>
                        </button>
                      </dd>
                    </div>
                    <div>
                      <dt>Contato</dt>
                      <dd>
                        <button type="button" onClick={() => void openUrl("mailto:llagrimante@gmail.com")}>
                          llagrimante@gmail.com
                          <span aria-hidden="true">↗</span>
                        </button>
                      </dd>
                    </div>
                  </dl>
                  <p className="info-local-note"><span aria-hidden="true">●</span> Privacidade por arquitetura · processamento local</p>
                </article>
              </section>
            )}

            {error && (
              <p className="collection-error" role="status">
                {error}
              </p>
            )}
          </div>
        </section>

        {showCloseDialog && (
          <div
            className="modal-backdrop"
            role="dialog"
            aria-modal="true"
            aria-labelledby="dialog-title"
          >
            <div className="modal-card">
              <p className="eyebrow">FINALIZAR OU MINIMIZAR</p>
              <h3 id="dialog-title">Você quer fechar mesmo ou quer ir para o tray icon?</h3>
              <p className="modal-desc">
                Ao ir para o tray icon, o Pulse continuará em execução na bandeja do sistema
                (no canto direito da barra de tarefas, junto aos ícones de internet e volume).
              </p>
              <div className="modal-actions">
                <button
                  className="button-primary"
                  onClick={() => void handleMinimizeToTray(true)}
                  autoFocus
                >
                  IR PARA O TRAY ICON
                </button>
                <button
                  className="button-danger"
                  onClick={() => void handleExitApp()}
                >
                  FECHAR MESMO
                </button>
                <button
                  className="button-secondary"
                  onClick={handleCancelClose}
                >
                  CANCELAR
                </button>
              </div>
            </div>
          </div>
        )}

        {processToClose && (
          <div className="modal-backdrop" role="dialog" aria-modal="true" aria-labelledby="process-dialog-title">
            <div className="modal-card process-modal-card">
              <p className="eyebrow">ENCERRAR APLICATIVO</p>
              <h3 id="process-dialog-title">Fechar {processToClose.displayName || processToClose.name}?</h3>
              <p className="modal-desc">
                {isWslHostProcess(processToClose)
                  ? "O Pulse desligará o WSL, encerrando todas as distribuições e processos Linux em execução. Trabalho não salvo pode ser perdido."
                  : `O Windows encerrará todos os ${processToClose.processCount ?? 1} processos deste grupo${(processToClose.processCount ?? 1) === 1 ? ` (PID ${processToClose.id})` : " ao mesmo tempo"}. Se necessário, o Pulse usará o encerramento forçado; trabalho não salvo pode ser perdido.`}
              </p>
              <div className="modal-actions">
                <button className="button-danger" onClick={() => void handleTerminateProcess()} disabled={terminatingProcessId != null} autoFocus>
                  {terminatingProcessId != null ? "FECHANDO…" : isWslHostProcess(processToClose) ? "DESLIGAR WSL" : "FECHAR APLICATIVO"}
                </button>
                <button className="button-secondary" onClick={() => setProcessToClose(null)} disabled={terminatingProcessId != null}>
                  CANCELAR
                </button>
              </div>
            </div>
          </div>
        )}

        <ToastNotification toast={toast} onDismiss={() => setToast(null)} />
      </main>
    </CrashBoundary>
  );
}

export default App;
