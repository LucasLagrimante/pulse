import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  CircleHelp,
  Copy,
  Cpu,
  Lock,
  Monitor,
  Network,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Terminal,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { NavigationItem } from "./App";
import "./SecurityHub.css";

/* ------------------------------------------------------------------ */
/* Tipos espelhados do Rust (serde camelCase / snake_case)             */
/* ------------------------------------------------------------------ */

export type AuditVerdict =
  | "active_remote_session"
  | "remote_surface_present"
  | "wslg_only"
  | "clean"
  | "partial";

export type CheckStatus = "ok" | "attention" | "alert" | "unavailable";

export type ProcessClassification =
  | "wslg_internal"
  | "rdp_service"
  | "rdp_client"
  | "remote_agent";

export type ConnectionDirection =
  | "inbound_rdp"
  | "listening_rdp"
  | "listening_remote_port"
  | "remote_agent_established"
  | "remote_agent_listening";

export type RdpConfiguration = {
  enabled: boolean | null;
  configuredPort: number | null;
  isCustomPort: boolean;
  nlaRequired: boolean | null;
  serviceStatus: string | null;
  servicePid: number | null;
  unavailableReason: string | null;
};

export type RemoteSessionEntry = {
  sessionName: string;
  username: string | null;
  sessionId: number;
  state: string;
  isRemote: boolean;
  isActiveRemote: boolean;
};

export type RemoteNetworkEntry = {
  protocol: string;
  direction: ConnectionDirection;
  localAddress: string;
  localPort: number;
  remoteAddress: string | null;
  remotePort: number | null;
  isLoopback: boolean;
  isPublicBind: boolean;
  pid: number;
  processName: string;
  executablePath: string | null;
  reason: string;
};

export type RemoteProcessEntry = {
  pid: number;
  name: string;
  executablePath: string | null;
  classification: ProcessClassification;
  label: string;
  explanation: string;
  externalConnections: number;
  listeningPorts: number[];
};

export type AuditCheckResult = {
  id: string;
  title: string;
  status: CheckStatus;
  summary: string;
  details: string[];
};

export type RemoteAccessAudit = {
  verdict: AuditVerdict;
  verdictTitle: string;
  verdictSummary: string;
  rdpConfig: RdpConfiguration;
  sessions: RemoteSessionEntry[];
  sessionsUnavailableReason: string | null;
  connections: RemoteNetworkEntry[];
  connectionsUnavailableReason: string | null;
  processes: RemoteProcessEntry[];
  processesUnavailableReason: string | null;
  checks: AuditCheckResult[];
  collectedAtUnixMs: number;
  elevated: boolean;
};

/* ------------------------------------------------------------------ */
/* Helpers                                                             */
/* ------------------------------------------------------------------ */

const CHECK_STATUS_LABEL: Record<CheckStatus, string> = {
  ok: "Em ordem",
  attention: "Atenção",
  alert: "Ativo agora",
  unavailable: "Não medido",
};

const PROCESS_CLASS_LABEL: Record<ProcessClassification, string> = {
  wslg_internal: "WSLg interno",
  rdp_service: "Serviço RDP",
  rdp_client: "Cliente de saída",
  remote_agent: "Agente remoto",
};

function formatTime(unixMs: number): string {
  if (!Number.isFinite(unixMs) || unixMs <= 0) return "";
  try {
    return new Date(unixMs).toLocaleTimeString("pt-BR");
  } catch {
    return "";
  }
}

function verdictTone(verdict: AuditVerdict): "ok" | "attention" | "alert" | "unavailable" {
  switch (verdict) {
    case "active_remote_session":
      return "alert";
    case "remote_surface_present":
      return "attention";
    case "partial":
      return "unavailable";
    default:
      return "ok";
  }
}

function renderVerdictIcon(verdict: AuditVerdict) {
  switch (verdict) {
    case "active_remote_session":
      return <ShieldAlert size={22} aria-hidden="true" />;
    case "remote_surface_present":
      return <AlertTriangle size={22} aria-hidden="true" />;
    case "partial":
      return <CircleHelp size={22} aria-hidden="true" />;
    default:
      return <ShieldCheck size={22} aria-hidden="true" />;
  }
}

function renderCheckIcon(status: CheckStatus) {
  switch (status) {
    case "ok":
      return <CheckCircle2 size={15} aria-hidden="true" />;
    case "attention":
      return <AlertTriangle size={15} aria-hidden="true" />;
    case "alert":
      return <ShieldAlert size={15} aria-hidden="true" />;
    default:
      return <CircleHelp size={15} aria-hidden="true" />;
  }
}

function formatAuditReportText(audit: RemoteAccessAudit): string {
  const lines: string[] = [
    "Pulse — Auditoria Determinística de Acesso Remoto",
    `Veredito: ${audit.verdictTitle}`,
    audit.verdictSummary,
    `Coletado em: ${formatTime(audit.collectedAtUnixMs)}`,
    "",
    "Verificações:",
  ];

  for (const check of audit.checks) {
    lines.push(`- [${CHECK_STATUS_LABEL[check.status]}] ${check.title}: ${check.summary}`);
    for (const detail of check.details) {
      lines.push(`    • ${detail}`);
    }
  }

  return lines.join("\n");
}

/* ------------------------------------------------------------------ */
/* Componente                                                          */
/* ------------------------------------------------------------------ */

export function SecurityHub({
  onNavigate,
}: {
  onNavigate?: (area: NavigationItem) => void;
}) {
  const [audit, setAudit] = useState<RemoteAccessAudit | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const runAudit = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<RemoteAccessAudit>("run_remote_access_audit");
      setAudit(result);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void runAudit();
  }, [runAudit]);

  const handleCopyReport = useCallback(async () => {
    if (!audit) return;
    try {
      await navigator.clipboard.writeText(formatAuditReportText(audit));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard indisponível em alguns ambientes; falha silenciosa.
    }
  }, [audit]);

  const summary = useMemo(() => {
    if (!audit) return null;
    const activeRemoteSessions = audit.sessions.filter(s => s.isActiveRemote).length;
    const remoteAgents = audit.processes.filter(p => p.classification === "remote_agent").length;
    const wslgComponents = audit.processes.filter(p => p.classification === "wslg_internal").length;
    const externalConns = audit.connections.filter(
      c =>
        !c.isLoopback &&
        (c.direction === "inbound_rdp" || c.direction === "remote_agent_established")
    ).length;
    const listeningConns = audit.connections.filter(
      c =>
        c.direction === "listening_rdp" ||
        c.direction === "listening_remote_port" ||
        c.direction === "remote_agent_listening"
    ).length;

    return {
      activeRemoteSessions,
      remoteAgents,
      wslgComponents,
      externalConns,
      listeningConns,
    };
  }, [audit]);

  const rdpLabel = useMemo(() => {
    if (!audit) return "—";
    if (audit.rdpConfig.unavailableReason != null || audit.rdpConfig.enabled == null) {
      return "Não medido";
    }
    return audit.rdpConfig.enabled ? "Habilitado" : "Desabilitado";
  }, [audit]);

  const rdpHint = useMemo(() => {
    if (!audit) return "verificando registro";
    if (audit.rdpConfig.unavailableReason != null) return "leitura bloqueada";
    const port = audit.rdpConfig.configuredPort;
    if (port != null) {
      return audit.rdpConfig.isCustomPort
        ? `porta customizada ${port}`
        : `porta padrão ${port}`;
    }
    return audit.rdpConfig.serviceStatus ?? "sem porta registrada";
  }, [audit]);

  return (
    <section className="module-dashboard sec-hub" aria-label="Segurança — Auditoria de acesso remoto">
      {error ? (
        <div className="sec-banner sec-banner-alert" role="alert">
          <AlertTriangle size={16} aria-hidden="true" />
          <span>Falha ao executar auditoria: {error}</span>
        </div>
      ) : null}

      {/* ---------------- Veredito Determinístico ---------------- */}
      {audit ? (
        <article
          className={`sec-verdict sec-verdict-${verdictTone(audit.verdict)}`}
          aria-live="polite"
        >
          <div className="sec-verdict-main">
            <span className={`sec-verdict-icon sec-tone-${verdictTone(audit.verdict)}`}>
              {renderVerdictIcon(audit.verdict)}
            </span>
            <div className="sec-verdict-copy">
              <span className="sec-verdict-eyebrow">DIAGNÓSTICO DETERMINÍSTICO · ACESSO REMOTO</span>
              <h3>{audit.verdictTitle}</h3>
              <p>{audit.verdictSummary}</p>
            </div>
          </div>
          <div className="sec-verdict-actions">
            <button
              type="button"
              className="sec-btn sec-btn-primary"
              onClick={() => void runAudit()}
              disabled={loading}
            >
              <RefreshCw size={14} aria-hidden="true" className={loading ? "sec-spin" : ""} />
              {loading ? "Verificando…" : "Verificar agora"}
            </button>
            <button
              type="button"
              className="sec-btn"
              onClick={() => void handleCopyReport()}
            >
              {copied ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
              {copied ? "Copiado" : "Copiar diagnóstico"}
            </button>
            <span className="sec-stamp">
              {formatTime(audit.collectedAtUnixMs)
                ? `atualizado ${formatTime(audit.collectedAtUnixMs)}`
                : ""}
            </span>
          </div>
        </article>
      ) : (
        <div className="sec-loading-card" role="status">
          <RefreshCw size={16} aria-hidden="true" className="sec-spin" />
          <span>Executando verificação determinística de RDP, sessões, conexões e processos…</span>
        </div>
      )}

      {/* ---------------- KPIs ---------------- */}
      <div className="sec-summary">
        <article className="sec-metric">
          <span className="sec-metric-label">
            <Lock size={13} aria-hidden="true" /> RDP de entrada
          </span>
          <strong className="sec-metric-value">{rdpLabel}</strong>
          <span className="sec-metric-hint">{rdpHint}</span>
        </article>
        <article className="sec-metric">
          <span className="sec-metric-label">
            <Monitor size={13} aria-hidden="true" /> Sessões remotas ativas
          </span>
          <strong className="sec-metric-value">
            {audit?.sessionsUnavailableReason != null
              ? "—"
              : (summary?.activeRemoteSessions ?? "—")}
          </strong>
          <span className="sec-metric-hint">
            {audit?.sessionsUnavailableReason != null
              ? "não medido"
              : audit
                ? `de ${audit.sessions.length} sessão(ões) listada(s)`
                : "coletando"}
          </span>
        </article>
        <article className="sec-metric">
          <span className="sec-metric-label">
            <Network size={13} aria-hidden="true" /> Conexões remotas
          </span>
          <strong className="sec-metric-value">
            {audit?.connectionsUnavailableReason != null
              ? "—"
              : (summary?.externalConns ?? "—")}
          </strong>
          <span className="sec-metric-hint">
            {audit?.connectionsUnavailableReason != null
              ? "tabela indisponível"
              : summary
                ? `${summary.listeningConns} porta(s) remota(s) em escuta`
                : "coletando"}
          </span>
        </article>
        <article className="sec-metric">
          <span className="sec-metric-label">
            <Cpu size={13} aria-hidden="true" /> Agentes remotos
          </span>
          <strong className="sec-metric-value">
            {audit?.processesUnavailableReason != null
              ? "—"
              : (summary?.remoteAgents ?? "—")}
          </strong>
          <span className="sec-metric-hint">
            {audit?.processesUnavailableReason != null
              ? "não medido"
              : summary
                ? summary.wslgComponents > 0
                  ? `${summary.wslgComponents} componente(s) WSLg`
                  : "nenhum agente ativo"
                : "coletando"}
          </span>
        </article>
      </div>

      {/* ---------------- 4 Verificações Determinísticas ---------------- */}
      <header className="sec-section-head">
        <h3>
          <ShieldCheck size={14} aria-hidden="true" /> Verificações determinísticas
        </h3>
        {audit && !audit.elevated ? (
          <span className="sec-hint-pill">
            <ShieldAlert size={12} aria-hidden="true" /> sem elevação (caminhos de processos protegidos podem ficar ocultos)
          </span>
        ) : null}
      </header>

      <div className="sec-checks">
        {audit?.checks.map(check => (
          <article
            key={check.id}
            className={`sec-check sec-check-${check.status}`}
            aria-label={check.title}
          >
            <div className="sec-check-head">
              <span className="sec-check-title">
                {renderCheckIcon(check.status)}
                {check.title}
              </span>
              <span className={`sec-badge sec-badge-${check.status}`}>
                {CHECK_STATUS_LABEL[check.status]}
              </span>
            </div>
            <p className="sec-check-summary">{check.summary}</p>
            {check.details.length > 0 ? (
              <ul className="sec-check-details">
                {check.details.map((item, idx) => (
                  <li key={`${check.id}-${idx}`}>{item}</li>
                ))}
              </ul>
            ) : null}
          </article>
        ))}
      </div>

      {/* ---------------- Processos detectados ---------------- */}
      <header className="sec-section-head">
        <h3>
          <Terminal size={14} aria-hidden="true" /> Processos de acesso remoto e WSLg
        </h3>
      </header>

      {audit?.processesUnavailableReason ? (
        <p className="sec-empty">{audit.processesUnavailableReason}</p>
      ) : audit && audit.processes.length > 0 ? (
        <div className="sec-list" aria-label="Processos de acesso remoto e WSLg">
          {audit.processes.map(proc => (
            <article key={proc.pid} className="sec-item">
              <div className="sec-item-top">
                <div className="sec-item-id">
                  <strong>{proc.name}</strong>
                  <span className="sec-mono">PID {proc.pid}</span>
                </div>
                <span className={`sec-class-pill sec-class-${proc.classification}`}>
                  {PROCESS_CLASS_LABEL[proc.classification]}
                </span>
              </div>
              <p className="sec-item-label">{proc.label}</p>
              <p className="sec-item-desc">{proc.explanation}</p>
              <div className="sec-item-meta">
                <span className="sec-mono sec-path">
                  {proc.executablePath ?? "Caminho protegido pelo sistema"}
                </span>
                <span className="sec-mono">
                  {proc.externalConnections} externa(s)
                  {proc.listeningPorts.length > 0
                    ? ` · escuta: ${proc.listeningPorts.join(", ")}`
                    : ""}
                </span>
              </div>
            </article>
          ))}
        </div>
      ) : audit ? (
        <p className="sec-empty">
          Nenhum agente de acesso remoto, cliente RDP ou componente WSLg em execução neste momento.
        </p>
      ) : null}

      {/* ---------------- Conexões relevantes ---------------- */}
      <header className="sec-section-head">
        <h3>
          <Network size={14} aria-hidden="true" /> Conexões e portas de acesso remoto
        </h3>
      </header>

      {audit?.connectionsUnavailableReason ? (
        <p className="sec-empty">{audit.connectionsUnavailableReason}</p>
      ) : audit && audit.connections.length > 0 ? (
        <div className="sec-list" aria-label="Conexões e portas de acesso remoto">
          {audit.connections.map((conn, idx) => (
            <article key={`${conn.pid}-${conn.localPort}-${idx}`} className="sec-item">
              <div className="sec-item-top">
                <div className="sec-item-id">
                  <strong>{conn.processName}</strong>
                  <span className="sec-mono">PID {conn.pid}</span>
                </div>
                <span
                  className={`sec-badge ${
                    conn.direction === "inbound_rdp" && !conn.isLoopback
                      ? "sec-badge-alert"
                      : "sec-badge-attention"
                  }`}
                >
                  {conn.remoteAddress != null ? "ESTABELECIDA" : "EM ESCUTA"}
                </span>
              </div>
              <p className="sec-item-desc">{conn.reason}</p>
              <div className="sec-item-meta">
                <span className="sec-mono">
                  {conn.localAddress}:{conn.localPort}
                  {conn.remoteAddress != null && conn.remotePort != null
                    ? ` ↔ ${conn.remoteAddress}:${conn.remotePort}`
                    : ""}
                </span>
                {conn.isPublicBind ? (
                  <span className="sec-mono">alcançável pela rede local</span>
                ) : conn.isLoopback ? (
                  <span className="sec-mono">somente loopback local</span>
                ) : null}
              </div>
            </article>
          ))}
        </div>
      ) : audit ? (
        <p className="sec-empty">
          Nenhuma conexão TCP ativa ou porta em escuta encontrada na porta RDP configurada ou em portas conhecidas de acesso remoto.
        </p>
      ) : null}

      {/* ---------------- Sessões ---------------- */}
      <header className="sec-section-head">
        <h3>
          <Monitor size={14} aria-hidden="true" /> Sessões do sistema
        </h3>
      </header>

      {audit?.sessionsUnavailableReason ? (
        <p className="sec-empty">{audit.sessionsUnavailableReason}</p>
      ) : audit && audit.sessions.length > 0 ? (
        <div className="sec-sessions-grid" aria-label="Sessões do sistema">
          {audit.sessions.map(session => (
            <article
              key={`${session.sessionName}-${session.sessionId}`}
              className={`sec-session-card ${session.isActiveRemote ? "sec-session-alert" : ""}`}
            >
              <div className="sec-item-top">
                <strong>{session.sessionName}</strong>
                <span
                  className={`sec-badge ${
                    session.isActiveRemote
                      ? "sec-badge-alert"
                      : session.isRemote
                        ? "sec-badge-attention"
                        : "sec-badge-ok"
                  }`}
                >
                  {session.isRemote ? "Remota" : "Local"} · {session.state}
                </span>
              </div>
              <div className="sec-item-meta">
                <span>Usuário: {session.username ?? "—"}</span>
                <span className="sec-mono">ID {session.sessionId}</span>
              </div>
            </article>
          ))}
        </div>
      ) : audit ? (
        <p className="sec-empty">Nenhuma sessão retornada pelo sistema.</p>
      ) : null}

      {/* ---------------- Auditoria complementar + transparência ---------------- */}
      {onNavigate ? (
        <div className="sec-complement">
          <span className="sec-complement-label">Auditoria complementar no Pulse:</span>
          <div className="sec-complement-actions">
            <button
              type="button"
              className="sec-chip-link"
              onClick={() => onNavigate("Serviços e atualizações")}
            >
              Revisar serviços
            </button>
            <button
              type="button"
              className="sec-chip-link"
              onClick={() => onNavigate("Inicialização e segurança")}
            >
              Programas na inicialização
            </button>
            <button
              type="button"
              className="sec-chip-link"
              onClick={() => onNavigate("Rede e segurança")}
            >
              Todas as conexões de rede
            </button>
            <button
              type="button"
              className="sec-chip-link"
              onClick={() => onNavigate("Desinstalar programas")}
            >
              Aplicativos instalados
            </button>
          </div>
        </div>
      ) : null}

      <p className="sec-disclaimer">
        Verificação 100% local e determinística (Registro/porta real do RDP, serviço TermService,
        tabela TCP do kernel, processos em execução e sessões do sistema). Mostra fatos verificáveis
        sem rotular executáveis como malware e não substitui um antivírus.
      </p>
    </section>
  );
}
