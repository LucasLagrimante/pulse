import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  CircleHelp,
  Globe,
  Lock,
  Network,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import "./NetworkSecurity.css";

/* ------------------------------------------------------------------ */
/* Tipos espelhados do Rust (serde camelCase)                          */
/* ------------------------------------------------------------------ */

export type RemoteEndpoint = {
  address: string;
  port: number;
  isIpv6: boolean;
};

export type ProcessTraffic = {
  pid: number;
  /** `null` quando o processo é protegido (PPL/elevado) e o Pulse não está elevado. */
  executablePath: string | null;
  processName: string;
  establishedCount: number;
  udpSocketCount: number;
  listeningCount: number;
  /** `true` quando alguma porta escuta em `0.0.0.0`/`::` (alcançável pela LAN). */
  listensOnAllInterfaces: boolean;
  destinations: RemoteEndpoint[];
  externalTotal: number;
};

export type NetworkSnapshot = {
  processes: ProcessTraffic[];
  establishedTotal: number;
  establishedExternal: number;
  establishedLoopback: number;
  udpTotal: number;
  listeningTotal: number;
  listeningPublicTotal: number;
  /** Preenchido quando a tabela não pôde ser lida. Nunca é "zero conexões". */
  unavailableReason: string | null;
  collectedAtUnixMs: number;
};

export type SignalSeverity = "ok" | "attention" | "unavailable";

export type PostureSignal = {
  id: string;
  title: string;
  severity: SignalSeverity;
  summary: string;
  rationale: string;
  evidence: string[];
};

export type SecurityPosture = {
  signals: PostureSignal[];
  attentionCount: number;
  unavailableCount: number;
  collectedAtUnixMs: number;
  /** `true` quando o processo do Pulse tem token elevado. */
  elevated: boolean;
};

/* ------------------------------------------------------------------ */
/* Helpers                                                             */
/* ------------------------------------------------------------------ */

const CONNECTIONS_REFRESH_MS = 3000;

function formatTime(unixMs: number): string {
  try {
    return new Date(unixMs).toLocaleTimeString("pt-BR");
  } catch {
    return "";
  }
}

function severityIcon(severity: SignalSeverity) {
  switch (severity) {
    case "ok":
      return <CheckCircle2 size={16} aria-hidden="true" />;
    case "attention":
      return <AlertTriangle size={16} aria-hidden="true" />;
    default:
      return <CircleHelp size={16} aria-hidden="true" />;
  }
}

const SEVERITY_LABEL: Record<SignalSeverity, string> = {
  ok: "Em ordem",
  attention: "Atenção",
  unavailable: "Não medido",
};

/* ------------------------------------------------------------------ */
/* Componente                                                          */
/* ------------------------------------------------------------------ */

export function NetworkSecurity() {
  const [network, setNetwork] = useState<NetworkSnapshot | null>(null);
  const [posture, setPosture] = useState<SecurityPosture | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingPosture, setLoadingPosture] = useState(true);
  const [filter, setFilter] = useState<"external" | "listening" | "all">("external");
  const [expanded, setExpanded] = useState<number | null>(null);

  const loadNetwork = useCallback(async () => {
    try {
      const snapshot = await invoke<NetworkSnapshot>("get_network_connections");
      setNetwork(snapshot);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  const loadPosture = useCallback(async (force = false) => {
    setLoadingPosture(true);
    try {
      const next = await invoke<SecurityPosture>("get_security_posture", { forceRefresh: force });
      setPosture(next);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoadingPosture(false);
    }
  }, []);

  useEffect(() => {
    void loadNetwork();
    void loadPosture();
    const timer = window.setInterval(() => void loadNetwork(), CONNECTIONS_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [loadNetwork, loadPosture]);

  const processes = useMemo(() => {
    if (!network) return [];
    const list = network.processes;
    if (filter === "external") {
      return list.filter(process => process.externalTotal > 0);
    }
    if (filter === "listening") {
      return list.filter(process => process.listeningCount > 0);
    }
    return list;
  }, [network, filter]);

  const attentionSignals = posture?.signals.filter(signal => signal.severity === "attention") ?? [];

  return (
    <section className="module-dashboard ns-hub" aria-label="Rede e segurança">
      {error ? (
        <div className="ns-banner ns-banner-error" role="alert">
          <AlertTriangle size={16} aria-hidden="true" />
          <span>{error}</span>
        </div>
      ) : null}

      {network?.unavailableReason ? (
        <div className="ns-banner ns-banner-warn" role="status">
          <CircleHelp size={16} aria-hidden="true" />
          <span>{network.unavailableReason}</span>
        </div>
      ) : null}

      {/* ---------------- Resumo ---------------- */}
      <div className="ns-summary">
        <article className="ns-metric">
          <span className="ns-metric-label">
            <Network size={13} aria-hidden="true" /> Conexões externas
          </span>
          <strong className="ns-metric-value">{network?.establishedExternal ?? "—"}</strong>
          <span className="ns-metric-hint">
            de {network?.establishedTotal ?? "—"} estabelecidas
          </span>
        </article>
        <article className="ns-metric">
          <span className="ns-metric-label">
            <Activity size={13} aria-hidden="true" /> Loopback
          </span>
          <strong className="ns-metric-value">{network?.establishedLoopback ?? "—"}</strong>
          <span className="ns-metric-hint">só neste micro</span>
        </article>
        <article className="ns-metric">
          <span className="ns-metric-label">
            <Globe size={13} aria-hidden="true" /> Portas expostas
          </span>
          <strong className="ns-metric-value">{network?.listeningPublicTotal ?? "—"}</strong>
          <span className="ns-metric-hint">
            de {network?.listeningTotal ?? "—"} em escuta
          </span>
        </article>
        <article className="ns-metric">
          <span className="ns-metric-label">
            <ShieldCheck size={13} aria-hidden="true" /> Sinais de atenção
          </span>
          <strong className="ns-metric-value">{posture?.attentionCount ?? "—"}</strong>
          <span className="ns-metric-hint">
            {posture ? `${posture.unavailableCount} não medidos` : "coletando"}
          </span>
        </article>
      </div>

      {/* ---------------- Postura ---------------- */}
      <header className="ns-section-head">
        <h3>
          <Lock size={14} aria-hidden="true" /> Postura do sistema
        </h3>
        <div className="ns-head-actions">
          {posture && !posture.elevated ? (
            <span className="ns-hint-pill" title="Alguns sinais exigem privilégio administrativo">
              <ShieldAlert size={12} aria-hidden="true" /> sem privilégio elevado
            </span>
          ) : null}
          <button
            type="button"
            className="ns-icon-btn"
            onClick={() => void loadPosture(true)}
            disabled={loadingPosture}
            aria-label="Recoletar sinais de postura"
          >
            <RefreshCw size={14} aria-hidden="true" className={loadingPosture ? "ns-spin" : ""} />
          </button>
        </div>
      </header>

      {attentionSignals.length > 0 ? (
        <div className="ns-banner ns-banner-warn" role="status">
          <AlertTriangle size={16} aria-hidden="true" />
          <span>
            {attentionSignals.length === 1
              ? `1 ponto exige atenção: ${attentionSignals[0].title}.`
              : `${attentionSignals.length} pontos exigem atenção: ${attentionSignals
                  .map(signal => signal.title)
                  .join(", ")}.`}
          </span>
        </div>
      ) : null}

      <div className="ns-signals">
        {posture?.signals.map(signal => (
          <article
            key={signal.id}
            className={`ns-signal ns-signal-${signal.severity}`}
            aria-label={signal.title}
          >
            <div className="ns-signal-head">
              <span className="ns-signal-title">
                {severityIcon(signal.severity)}
                {signal.title}
              </span>
              <span className={`ns-severity ns-severity-${signal.severity}`}>
                {SEVERITY_LABEL[signal.severity]}
              </span>
            </div>
            <p className="ns-signal-summary">{signal.summary}</p>
            {signal.evidence.length > 0 ? (
              <ul className="ns-signal-evidence">
                {signal.evidence.map((item, index) => (
                  <li key={`${signal.id}-${index}`}>{item}</li>
                ))}
              </ul>
            ) : null}
            <p className="ns-signal-rationale">{signal.rationale}</p>
          </article>
        ))}
        {!posture && loadingPosture ? (
          <p className="ns-empty">Coletando sinais de postura…</p>
        ) : null}
      </div>

      {/* ---------------- Conexões ---------------- */}
      <header className="ns-section-head">
        <h3>
          <Network size={14} aria-hidden="true" /> Saída por processo
        </h3>
        <div className="ns-filters" role="group" aria-label="Filtrar processos">
          {(
            [
              ["external", "Com saída"],
              ["listening", "Em escuta"],
              ["all", "Todos"],
            ] as const
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              className={`ns-chip ${filter === value ? "active" : ""}`}
              onClick={() => setFilter(value)}
              aria-pressed={filter === value}
            >
              {label}
            </button>
          ))}
          <span className="ns-stamp">
            {network ? `atualizado ${formatTime(network.collectedAtUnixMs)}` : ""}
          </span>
        </div>
      </header>

      {processes.length === 0 ? (
        <p className="ns-empty">
          {network?.unavailableReason
            ? "Sem dados de rede para exibir."
            : filter === "external"
              ? "Nenhum processo com conexão externa neste instante."
              : "Nenhum processo nesta categoria."}
        </p>
      ) : (
        <div className="ns-table" aria-label="Conexões por processo">
          {/* Cabeçalho visual. Não é uma tabela ARIA: cada linha é um acordeão,
              então a semântica correta é lista de botões com aria-expanded —
              e cada botão carrega os números no próprio rótulo acessível. */}
          <div className="ns-row ns-row-head" aria-hidden="true">
            <span>Processo</span>
            <span>Saída</span>
            <span>Escuta</span>
            <span>UDP</span>
            <span>Destinos</span>
          </div>
          {processes.map(process => {
            const isOpen = expanded === process.pid;
            const rotulo =
              `${process.processName}, PID ${process.pid}: ` +
              `${process.externalTotal} conexão(ões) de saída, ` +
              `${process.listeningCount} porta(s) em escuta` +
              `${process.listensOnAllInterfaces ? " (exposta na rede local)" : ""}, ` +
              `${process.udpSocketCount} socket(s) UDP`;
            return (
              <div key={process.pid} className="ns-row-group">
                <button
                  type="button"
                  className={`ns-row ${isOpen ? "expanded" : ""}`}
                  onClick={() => setExpanded(isOpen ? null : process.pid)}
                  aria-expanded={isOpen}
                  aria-label={rotulo}
                >
                  <span className="ns-proc">
                    <span className="ns-proc-name" title={process.executablePath ?? undefined}>
                      {process.processName}
                    </span>
                    <span className="ns-proc-pid">PID {process.pid}</span>
                  </span>
                  <span className="ns-num">
                    {process.externalTotal > 0 ? process.externalTotal : "—"}
                  </span>
                  <span className="ns-num">
                    {process.listeningCount > 0 ? (
                      <>
                        {process.listeningCount}
                        {process.listensOnAllInterfaces ? (
                          <span
                            className="ns-public-dot"
                            title="Escuta em todas as interfaces (alcançável pela rede local)"
                          />
                        ) : null}
                      </>
                    ) : (
                      "—"
                    )}
                  </span>
                  <span className="ns-num">
                    {process.udpSocketCount > 0 ? process.udpSocketCount : "—"}
                  </span>
                  <span className="ns-dest-preview">
                    {process.destinations.length === 0
                      ? "—"
                      : process.destinations
                          .slice(0, 2)
                          .map(endpoint => `${endpoint.address}:${endpoint.port}`)
                          .join(", ")}
                    {process.destinations.length > 2 ? " …" : ""}
                  </span>
                </button>
                {isOpen ? (
                  <div className="ns-detail">
                    {process.executablePath ? (
                      <p className="ns-detail-path">{process.executablePath}</p>
                    ) : (
                      <p className="ns-detail-path ns-detail-muted">
                        Caminho indisponível: processo protegido pelo Windows ou Pulse sem
                        privilégio elevado.
                      </p>
                    )}
                    {process.destinations.length > 0 ? (
                      <ul className="ns-endpoints">
                        {process.destinations.map(endpoint => (
                          <li key={`${process.pid}-${endpoint.address}-${endpoint.port}`}>
                            <span className="ns-endpoint-addr">
                              {endpoint.isIpv6 ? `[${endpoint.address}]` : endpoint.address}
                            </span>
                            <span className="ns-endpoint-port">:{endpoint.port}</span>
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <p className="ns-detail-muted">Sem destino externo registrado.</p>
                    )}
                    {process.externalTotal > process.destinations.length ? (
                      <p className="ns-detail-muted">
                        {process.externalTotal - process.destinations.length} destinos adicionais não
                        listados.
                      </p>
                    ) : null}
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      )}

      <p className="ns-disclaimer">
        O Pulse mostra o que foi medido por APIs do próprio Windows. Ele não classifica programas
        como malware e não substitui o Microsoft Defender.
      </p>
    </section>
  );
}
