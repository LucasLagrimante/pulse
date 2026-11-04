import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { CachedDiskScan } from "./diskScanStore";
import { ProviderIcon } from "./ProviderIcon";

export type DiskItemSummary = {
  path: string;
  name: string;
  sizeBytes: number;
  sizeFormatted: string;
  isDir: boolean;
  fileCount: number;
};

export type AiDiskAnalysisPreview = {
  provider: "claude" | "antigravity" | "codex" | string;
  providerName: string;
  drivePath: string;
  command: string;
  prompt: string;
  contextFilePath: string;
  topDirectories: DiskItemSummary[];
  topFiles: DiskItemSummary[];
  cleanupCandidates: DiskItemSummary[];
  totalCandidatesBytes: number;
  totalCandidatesFormatted: string;
  isAvailable: boolean;
  environment: string;
};

export type AiTerminalLaunchResult = {
  success: boolean;
  provider: string;
  command: string;
  contextFilePath: string;
  terminalType: string;
  message: string;
};

export type AiCleanupPlanStatus = {
  path: string;
  exists: boolean;
  modifiedAtUnixMs: number | null;
  sizeBytes: number;
};

/** Intervalo de checagem do plano que a IA grava enquanto o modal está aberto. */
export const CLEANUP_PLAN_POLL_MS = 4000;

export type SupportedAiProvider = "antigravity" | "claude" | "codex";

export const AI_PROVIDERS: Array<{
  id: SupportedAiProvider;
  name: string;
  subtitle: string;
  cliCmd: string;
}> = [
  {
    id: "antigravity",
    name: "Antigravity",
    subtitle: "Google AI Pro · CLI agy",
    cliCmd: "agy",
  },
  {
    id: "claude",
    name: "Claude Code",
    subtitle: "Anthropic · CLI claude",
    cliCmd: "claude",
  },
  {
    id: "codex",
    name: "Codex CLI",
    subtitle: "OpenAI ChatGPT · CLI codex",
    cliCmd: "codex",
  },
];

export type AiDiskAdvisorModalProps = {
  isOpen: boolean;
  onClose: () => void;
  cachedScan: CachedDiskScan | null;
  /** Provedor pré-selecionado ao abrir (o que o botão estava mostrando). */
  initialProvider?: SupportedAiProvider;
};

export function AiDiskAdvisorModal({
  isOpen,
  onClose,
  cachedScan,
  initialProvider = "antigravity",
}: AiDiskAdvisorModalProps) {
  const [selectedProvider, setSelectedProvider] =
    useState<SupportedAiProvider>(initialProvider);
  const [preview, setPreview] = useState<AiDiskAnalysisPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [customPrompt, setCustomPrompt] = useState<string>("");
  const [isEditingPrompt, setIsEditingPrompt] = useState(false);

  const [launching, setLaunching] = useState(false);
  const [launchResult, setLaunchResult] =
    useState<AiTerminalLaunchResult | null>(null);
  const [copiedType, setCopiedType] = useState<"command" | "prompt" | null>(
    null,
  );

  const [planStatus, setPlanStatus] = useState<AiCleanupPlanStatus | null>(null);

  // A IA grava o plano no próprio ritmo: checa ao abrir e periodicamente
  useEffect(() => {
    if (!isOpen || !cachedScan) return;

    let isMounted = true;
    const refresh = () => {
      invoke<AiCleanupPlanStatus | null>("get_ai_cleanup_plan_status")
        .then(status => {
          if (isMounted) setPlanStatus(status ?? null);
        })
        .catch(() => {
          if (isMounted) setPlanStatus(null);
        });
    };
    refresh();
    const timer = window.setInterval(refresh, CLEANUP_PLAN_POLL_MS);
    return () => {
      isMounted = false;
      window.clearInterval(timer);
    };
  }, [isOpen, cachedScan]);

  // Cada abertura começa no provedor que o botão estava exibindo
  useEffect(() => {
    if (isOpen) {
      setSelectedProvider(initialProvider);
      setIsEditingPrompt(false);
    }
  }, [isOpen, initialProvider]);

  // Carrega a prévia sempre que o modal abre ou o provedor muda
  useEffect(() => {
    if (!isOpen || !cachedScan) return;

    let isMounted = true;
    setLoading(true);
    setError(null);
    setLaunchResult(null);

    invoke<AiDiskAnalysisPreview>("get_ai_disk_analysis_preview", {
      provider: selectedProvider,
      customPrompt: isEditingPrompt && customPrompt.trim() ? customPrompt : null,
    })
      .then(res => {
        if (!isMounted) return;
        setPreview(res);
        if (!isEditingPrompt) {
          setCustomPrompt(res.prompt);
        }
      })
      .catch(err => {
        if (!isMounted) return;
        setError(
          typeof err === "string"
            ? err
            : "Falha ao preparar os dados da análise com IA.",
        );
      })
      .finally(() => {
        if (isMounted) setLoading(false);
      });

    return () => {
      isMounted = false;
    };
  }, [isOpen, cachedScan, selectedProvider, isEditingPrompt]);

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
      const result = await invoke<AiTerminalLaunchResult>(
        "launch_ai_disk_terminal",
        {
          provider: selectedProvider,
          customPrompt: isEditingPrompt && customPrompt.trim() ? customPrompt : null,
        },
      );
      setLaunchResult(result);
    } catch (err) {
      setError(
        typeof err === "string"
          ? err
          : "Erro ao abrir a janela do terminal no sistema.",
      );
    } finally {
      setLaunching(false);
    }
  };

  const handleOpenPlan = async (reveal: boolean) => {
    if (!planStatus?.exists) return;
    try {
      if (reveal) {
        await invoke("open_disk_item", { path: planStatus.path, reveal: true });
      } else {
        await invoke("open_ai_cleanup_plan");
      }
    } catch (err) {
      setError(typeof err === "string" ? err : "Não foi possível abrir o plano de limpeza.");
    }
  };

  const handleResetPrompt = () => {
    setIsEditingPrompt(false);
    if (preview) {
      setCustomPrompt(preview.prompt);
    }
  };

  return (
    <div
      className="modal-backdrop disk-ai-modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="disk-ai-modal-title"
      onClick={onClose}
    >
      <div
        className="modal-card disk-ai-modal-card"
        onClick={e => e.stopPropagation()}
      >
        {/* Cabeçalho */}
        <header className="disk-ai-modal-header">
          <div className="disk-ai-header-info">
            <span className="eyebrow">DIAGNÓSTICO COM IA VIA CLI</span>
            <h3 id="disk-ai-modal-title">
              Otimização de Armazenamento e Limpeza de Disco
            </h3>
            <p className="modal-desc">
              Envie o relatório da varredura diretamente para o terminal com{" "}
              <strong>Claude</strong>, <strong>Antigravity</strong> ou{" "}
              <strong>Codex</strong> para avaliar as maiores pastas e receber
              instruções práticas de liberação de espaço.
            </p>
          </div>
          <button
            type="button"
            className="disk-ai-close-btn"
            onClick={onClose}
            aria-label="Fechar diálogo"
          >
            ✕
          </button>
        </header>

        {/* Seletor dos 3 Provedores de Cotas */}
        <section
          className="disk-ai-providers-grid"
          role="radiogroup"
          aria-label="Escolher assistente de IA"
        >
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
                  {isSelected && preview && (
                    <span className="env-status-tag">{preview.environment}</span>
                  )}
                </div>
              </button>
            );
          })}
        </section>

        {/* Mensagens de Erro ou Sucesso */}
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

        {planStatus?.exists && (
          <div className="disk-ai-plan-card" role="status">
            <div className="disk-ai-plan-info">
              <span className="section-label">PLANO DE LIMPEZA GERADO PELA IA</span>
              <span className="section-hint">
                {planStatus.modifiedAtUnixMs
                  ? `Atualizado em ${new Date(planStatus.modifiedAtUnixMs).toLocaleString("pt-BR")}`
                  : "Arquivo disponível"}
                {" · "}
                <code className="context-path font-mono" title={planStatus.path}>
                  {planStatus.path}
                </code>
              </span>
            </div>
            <div className="disk-ai-plan-actions">
              <button
                type="button"
                className="text-link-btn"
                onClick={() => void handleOpenPlan(true)}
              >
                Mostrar na Pasta
              </button>
              <button
                type="button"
                className="button-primary"
                onClick={() => void handleOpenPlan(false)}
              >
                ABRIR PLANO
              </button>
            </div>
          </div>
        )}

        {/* Conteúdo com Resumo e Visualização */}
        <div className="disk-ai-modal-body">
          {loading && !preview ? (
            <div className="disk-ai-loading">
              <span className="scan-spinner" aria-hidden="true" />
              <span>Preparando contexto e analisando maiores pastas...</span>
            </div>
          ) : preview ? (
            <>
              {/* Cards de Métricas da Varredura */}
              <div className="disk-ai-summary-grid">
                <div className="summary-stat-card">
                  <span className="stat-label">UNIDADE ANALISADA</span>
                  <strong className="stat-value font-mono">
                    {preview.drivePath}
                  </strong>
                  <span className="stat-footnote">
                    Varredura recente em cache
                  </span>
                </div>

                <div className="summary-stat-card">
                  <span className="stat-label">MAIORES PASTAS</span>
                  <strong className="stat-value font-mono">
                    {preview.topDirectories.length > 0
                      ? preview.topDirectories[0].sizeFormatted
                      : "—"}
                  </strong>
                  <span className="stat-footnote" title={preview.topDirectories[0]?.path}>
                    {preview.topDirectories[0]?.name || "Nenhuma pasta"}
                  </span>
                </div>

                <div className="summary-stat-card highlight">
                  <span className="stat-label">RESÍDUOS / CACHES ESTIMADOS</span>
                  <strong className="stat-value font-mono">
                    {preview.totalCandidatesFormatted}
                  </strong>
                  <span className="stat-footnote">
                    {preview.cleanupCandidates.length} itens detectados
                  </span>
                </div>
              </div>

              {/* Lista Rápida dos Maiores Itens */}
              <div className="disk-ai-details-split">
                <div className="details-col">
                  <div className="details-head">
                    <span>TOP 5 PASTAS MAIS PESADAS</span>
                  </div>
                  <ul className="details-list font-mono">
                    {preview.topDirectories.slice(0, 5).map((dir, idx) => (
                      <li key={dir.path} title={dir.path}>
                        <span className="rank-num">#{idx + 1}</span>
                        <span className="item-name">{dir.name}</span>
                        <span className="item-size">{dir.sizeFormatted}</span>
                      </li>
                    ))}
                  </ul>
                </div>

                <div className="details-col">
                  <div className="details-head">
                    <span>POTENCIAIS ALVOS DE LIMPEZA</span>
                  </div>
                  <ul className="details-list font-mono">
                    {preview.cleanupCandidates.slice(0, 5).map((cand, idx) => (
                      <li key={cand.path} title={cand.path}>
                        <span className="rank-num">#{idx + 1}</span>
                        <span className="item-name">{cand.name}</span>
                        <span className="item-size">{cand.sizeFormatted}</span>
                      </li>
                    ))}
                    {preview.cleanupCandidates.length === 0 && (
                      <li className="empty-item">Nenhum resíduo óbvio detectado</li>
                    )}
                  </ul>
                </div>
              </div>

              {/* Pré-Prompt Contextual */}
              <div className="disk-ai-section">
                <div className="section-head">
                  <div className="section-title-group">
                    <span className="section-label">PRÉ-PROMPT PARA A IA</span>
                    <span className="section-hint">
                      Focado em identificar maiores pastas e onde liberar espaço
                    </span>
                  </div>
                  <div className="section-actions">
                    {isEditingPrompt ? (
                      <button
                        type="button"
                        className="text-link-btn"
                        onClick={handleResetPrompt}
                      >
                        Restaurar Padrão
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="text-link-btn"
                        onClick={() => setIsEditingPrompt(true)}
                      >
                        Editar Prompt
                      </button>
                    )}
                    <button
                      type="button"
                      className="text-link-btn"
                      onClick={() => handleCopy(customPrompt, "prompt")}
                    >
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
                    aria-label="Editar pré-prompt de análise"
                  />
                ) : (
                  <p className="disk-ai-prompt-preview font-mono">
                    {customPrompt}
                  </p>
                )}
              </div>

              {/* Comando de Terminal Pronto */}
              <div className="disk-ai-section">
                <div className="section-head">
                  <div className="section-title-group">
                    <span className="section-label">COMANDO GERADO PARA O TERMINAL</span>
                    <span className="section-hint">
                      Arquivo de contexto salvo em:{" "}
                      <code className="context-path font-mono" title={preview.contextFilePath}>
                        {preview.contextFilePath}
                      </code>
                    </span>
                  </div>
                  <button
                    type="button"
                    className="text-link-btn"
                    onClick={() => handleCopy(preview.command, "command")}
                  >
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

        {/* Rodapé com Botões de Ação */}
        <footer className="disk-ai-modal-footer">
          <div className="footer-left">
            {preview && (
              <span className="footer-env-note">
                Ambiente: <strong>{preview.environment}</strong>
              </span>
            )}
          </div>
          <div className="footer-actions">
            <button
              type="button"
              className="button-secondary"
              onClick={onClose}
              disabled={launching}
            >
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
