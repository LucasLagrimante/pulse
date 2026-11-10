import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import {
  AI_PROVIDERS,
  CLEANUP_PLAN_POLL_MS,
  type AiCleanupPlanStatus,
  type AiTerminalLaunchResult,
  type SupportedAiProvider,
} from "./AiDiskAdvisorModal";
import { ProviderIcon } from "./ProviderIcon";

/** Espelha `ServiceSummary` do Rust (`ai_services_advisor.rs`). */
export type AiServiceSummary = {
  name: string;
  displayName: string;
  executablePath: string | null;
  startName: string | null;
  startType: string;
  processId: number | null;
  publisher: string | null;
  category: string;
  reasons: string[];
};

export type AiServicesAnalysisPreview = {
  provider: string;
  providerName: string;
  command: string;
  prompt: string;
  contextFilePath: string;
  runningCount: number;
  analyzedCount: number;
  reviewCount: number;
  withoutPublisherCount: number;
  highlighted: AiServiceSummary[];
  collectedAtUnixMs: number;
  isAvailable: boolean;
  environment: string;
};

export type AiServicesAdvisorModalProps = {
  isOpen: boolean;
  onClose: () => void;
  /** Provedor pré-selecionado ao abrir (o que o botão estava mostrando). */
  initialProvider?: SupportedAiProvider;
};

const errorText = (err: unknown, fallback: string) => (typeof err === "string" ? err : fallback);

/**
 * Revisão de segurança dos serviços ativos com IA via CLI. Mesmo molde do
 * assistente de disco: o Pulse só gera o relatório e o pré-prompt e abre o
 * terminal — nada é parado, desativado ou removido daqui.
 */
export function AiServicesAdvisorModal({
  isOpen,
  onClose,
  initialProvider = "antigravity",
}: AiServicesAdvisorModalProps) {
  const [selectedProvider, setSelectedProvider] = useState<SupportedAiProvider>(initialProvider);
  const [preview, setPreview] = useState<AiServicesAnalysisPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [customPrompt, setCustomPrompt] = useState("");
  const [isEditingPrompt, setIsEditingPrompt] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [launchResult, setLaunchResult] = useState<AiTerminalLaunchResult | null>(null);
  const [copiedType, setCopiedType] = useState<"command" | "prompt" | null>(null);
  const [reviewStatus, setReviewStatus] = useState<AiCleanupPlanStatus | null>(null);

  // A IA grava o parecer no próprio ritmo: checa ao abrir e periodicamente
  useEffect(() => {
    if (!isOpen) return;
    let isMounted = true;
    const refresh = () => {
      invoke<AiCleanupPlanStatus>("get_ai_services_review_status")
        .then(status => {
          if (isMounted) setReviewStatus(status ?? null);
        })
        .catch(() => {
          if (isMounted) setReviewStatus(null);
        });
    };
    refresh();
    const timer = window.setInterval(refresh, CLEANUP_PLAN_POLL_MS);
    return () => {
      isMounted = false;
      window.clearInterval(timer);
    };
  }, [isOpen]);

  // Cada abertura começa no provedor que o botão estava exibindo
  useEffect(() => {
    if (isOpen) {
      setSelectedProvider(initialProvider);
      setIsEditingPrompt(false);
    }
  }, [isOpen, initialProvider]);

  useEffect(() => {
    if (!isOpen) return;
    let isMounted = true;
    setLoading(true);
    setError(null);
    setLaunchResult(null);

    invoke<AiServicesAnalysisPreview>("get_ai_services_analysis_preview", {
      provider: selectedProvider,
      customPrompt: isEditingPrompt && customPrompt.trim() ? customPrompt : null,
    })
      .then(res => {
        if (!isMounted) return;
        setPreview(res);
        if (!isEditingPrompt) setCustomPrompt(res.prompt);
      })
      .catch(err => {
        if (isMounted) setError(errorText(err, "Falha ao preparar os dados da revisão com IA."));
      })
      .finally(() => {
        if (isMounted) setLoading(false);
      });

    return () => {
      isMounted = false;
    };
    // customPrompt fica de fora de propósito: a prévia não é refeita a cada tecla digitada.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, selectedProvider, isEditingPrompt]);

  if (!isOpen) return null;

  const handleProviderSelect = (providerId: SupportedAiProvider) => {
    setSelectedProvider(providerId);
    setIsEditingPrompt(false);
    setLaunchResult(null);
  };

  const handleCopy = async (text: string, type: "command" | "prompt") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedType(type);
      setTimeout(() => setCopiedType(null), 2500);
    } catch {
      setError("Não foi possível copiar para a área de transferência.");
    }
  };

  const handleLaunchTerminal = async () => {
    setLaunching(true);
    setError(null);
    setLaunchResult(null);
    try {
      const result = await invoke<AiTerminalLaunchResult>("launch_ai_services_terminal", {
        provider: selectedProvider,
        customPrompt: isEditingPrompt && customPrompt.trim() ? customPrompt : null,
      });
      setLaunchResult(result);
    } catch (err) {
      setError(errorText(err, "Erro ao abrir a janela do terminal no sistema."));
    } finally {
      setLaunching(false);
    }
  };

  const handleOpenReview = async (reveal: boolean) => {
    if (!reviewStatus?.exists) return;
    try {
      if (reveal) {
        await invoke("open_disk_item", { path: reviewStatus.path, reveal: true });
      } else {
        await invoke("open_ai_services_review");
      }
    } catch (err) {
      setError(errorText(err, "Não foi possível abrir o parecer de segurança."));
    }
  };

  const handleResetPrompt = () => {
    setIsEditingPrompt(false);
    if (preview) setCustomPrompt(preview.prompt);
  };

  return (
    <div
      className="modal-backdrop disk-ai-modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="services-ai-modal-title"
      onClick={onClose}
    >
      <div className="modal-card disk-ai-modal-card" onClick={e => e.stopPropagation()}>
        <header className="disk-ai-modal-header">
          <div className="disk-ai-header-info">
            <span className="eyebrow">REVISÃO DE SEGURANÇA COM IA VIA CLI</span>
            <h3 id="services-ai-modal-title">Serviços em execução sob suspeita</h3>
            <p className="modal-desc">
              Envie os serviços ativos para <strong>Claude</strong>, <strong>Antigravity</strong> ou{" "}
              <strong>Codex</strong> procurarem sinais de keylogger, spyware ou acesso indevido. O Pulse
              só gera o relatório e o prompt: a IA é instruída a apenas investigar, sem parar nem alterar nada.
            </p>
          </div>
          <button type="button" className="disk-ai-close-btn" onClick={onClose} aria-label="Fechar diálogo">
            ✕
          </button>
        </header>

        <section className="disk-ai-providers-grid" role="radiogroup" aria-label="Escolher assistente de IA">
          {AI_PROVIDERS.map(p => {
            const isSelected = selectedProvider === p.id;
            return (
              <button
                key={p.id}
                type="button"
                role="radio"
                aria-checked={isSelected}
                className={`disk-ai-provider-card ${isSelected ? "selected" : ""}`}
                onClick={() => handleProviderSelect(p.id)}
              >
                <div className="provider-card-head">
                  <div className="provider-icon-wrapper">
                    <ProviderIcon providerId={p.id} size={22} />
                  </div>
                  <div className="provider-info">
                    <strong>{p.name}</strong>
                    <span className="provider-subtitle">{p.subtitle}</span>
                  </div>
                </div>
                <div className="provider-card-badge">
                  <span className="cli-tag font-mono">{p.cliCmd}</span>
                  {isSelected && preview && <span className="env-status-tag">{preview.environment}</span>}
                </div>
              </button>
            );
          })}
        </section>

        {error && (
          <div className="disk-ai-alert error" role="alert">
            <span>⚠ {error}</span>
          </div>
        )}

        {launchResult && (
          <div className="disk-ai-alert success" role="status">
            <strong>✓ {launchResult.terminalType} iniciado!</strong>
            <span>{launchResult.message}</span>
          </div>
        )}

        {reviewStatus?.exists && (
          <div className="disk-ai-plan-card" role="status">
            <div className="disk-ai-plan-info">
              <span className="section-label">PARECER DE SEGURANÇA GERADO PELA IA</span>
              <span className="section-hint">
                {reviewStatus.modifiedAtUnixMs
                  ? `Atualizado em ${new Date(reviewStatus.modifiedAtUnixMs).toLocaleString("pt-BR")}`
                  : "Arquivo disponível"}
                {" · "}
                <code className="context-path font-mono" title={reviewStatus.path}>
                  {reviewStatus.path}
                </code>
              </span>
            </div>
            <div className="disk-ai-plan-actions">
              <button type="button" className="text-link-btn" onClick={() => void handleOpenReview(true)}>
                Mostrar na Pasta
              </button>
              <button type="button" className="button-primary" onClick={() => void handleOpenReview(false)}>
                ABRIR PARECER
              </button>
            </div>
          </div>
        )}

        <div className="disk-ai-modal-body">
          {loading && !preview ? (
            <div className="disk-ai-loading">
              <span className="scan-spinner" aria-hidden="true" />
              <span>Preparando o relatório dos serviços em execução...</span>
            </div>
          ) : preview ? (
            <>
              <div className="disk-ai-summary-grid">
                <div className="summary-stat-card">
                  <span className="stat-label">EM EXECUÇÃO</span>
                  <strong className="stat-value font-mono">{preview.runningCount}</strong>
                  <span className="stat-footnote">
                    Coletados em {new Date(preview.collectedAtUnixMs).toLocaleTimeString("pt-BR")}
                  </span>
                </div>
                <div className="summary-stat-card highlight">
                  <span className="stat-label">FORA DAS PASTAS DO WINDOWS</span>
                  <strong className="stat-value font-mono">{preview.analyzedCount}</strong>
                  <span className="stat-footnote">Revisados um a um pela IA</span>
                </div>
                <div className="summary-stat-card">
                  <span className="stat-label">SINAIS DO PULSE</span>
                  <strong className="stat-value font-mono">{preview.reviewCount}</strong>
                  <span className="stat-footnote">
                    {preview.withoutPublisherCount} sem fabricante declarado
                  </span>
                </div>
              </div>

              <div className="details-col">
                <div className="details-head">
                  <span>PRIMEIROS DA FILA DE REVISÃO</span>
                </div>
                <ul className="details-list font-mono">
                  {preview.highlighted.map((svc, idx) => (
                    <li key={svc.name} title={svc.executablePath ?? svc.name}>
                      <span className="rank-num">#{idx + 1}</span>
                      <span className="item-name">{svc.displayName || svc.name}</span>
                      <span className="item-size">{svc.publisher ?? "sem fabricante"}</span>
                    </li>
                  ))}
                  {preview.highlighted.length === 0 && (
                    <li className="empty-item">Todos os serviços ativos rodam das pastas do Windows</li>
                  )}
                </ul>
              </div>

              <div className="disk-ai-section">
                <div className="section-head">
                  <div className="section-title-group">
                    <span className="section-label">PRÉ-PROMPT PARA A IA</span>
                    <span className="section-hint">Somente investigação, com evidência e sem alterar o sistema</span>
                  </div>
                  <div className="section-actions">
                    {isEditingPrompt ? (
                      <button type="button" className="text-link-btn" onClick={handleResetPrompt}>
                        Restaurar Padrão
                      </button>
                    ) : (
                      <button type="button" className="text-link-btn" onClick={() => setIsEditingPrompt(true)}>
                        Editar Prompt
                      </button>
                    )}
                    <button type="button" className="text-link-btn" onClick={() => handleCopy(customPrompt, "prompt")}>
                      {copiedType === "prompt" ? "Copiado!" : "Copiar Prompt"}
                    </button>
                  </div>
                </div>
                {isEditingPrompt ? (
                  <textarea
                    className="disk-ai-prompt-input font-mono"
                    rows={4}
                    value={customPrompt}
                    onChange={e => setCustomPrompt(e.target.value)}
                    aria-label="Editar pré-prompt da revisão de segurança"
                  />
                ) : (
                  <p className="disk-ai-prompt-preview font-mono">{customPrompt}</p>
                )}
              </div>

              <div className="disk-ai-section">
                <div className="section-head">
                  <div className="section-title-group">
                    <span className="section-label">COMANDO GERADO PARA O TERMINAL</span>
                    <span className="section-hint">
                      Relatório salvo em:{" "}
                      <code className="context-path font-mono" title={preview.contextFilePath}>
                        {preview.contextFilePath}
                      </code>
                    </span>
                  </div>
                  <button type="button" className="text-link-btn" onClick={() => handleCopy(preview.command, "command")}>
                    {copiedType === "command" ? "✓ Copiado!" : "Copiar Comando"}
                  </button>
                </div>
                <div className="disk-ai-command-box">
                  <pre className="font-mono">{preview.command}</pre>
                </div>
              </div>
            </>
          ) : null}
        </div>

        <footer className="disk-ai-modal-footer">
          <div className="footer-left">
            {preview && (
              <span className="footer-env-note">
                Ambiente: <strong>{preview.environment}</strong>
              </span>
            )}
          </div>
          <div className="footer-actions">
            <button type="button" className="button-secondary" onClick={onClose} disabled={launching}>
              FECHAR
            </button>
            <button
              type="button"
              className="button-secondary"
              onClick={() => preview && handleCopy(preview.command, "command")}
              disabled={!preview || launching}
            >
              {copiedType === "command" ? "COMANDO COPIADO!" : "COPIAR COMANDO"}
            </button>
            <button
              type="button"
              className="button-primary disk-ai-launch-btn"
              onClick={() => void handleLaunchTerminal()}
              disabled={!preview || launching}
            >
              {launching ? (
                <>
                  <span className="scan-spinner" aria-hidden="true" />
                  ABRINDO TERMINAL...
                </>
              ) : (
                <>
                  <ProviderIcon providerId={selectedProvider} size={15} />
                  ABRIR NO TERMINAL
                </>
              )}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
