import { invoke } from "@tauri-apps/api/core";
import { useMemo, useState } from "react";
import { AutoCleanupPanel } from "./AutoCleanupPanel";
import { useIsStandardUser } from "./PrivilegeIndicator";

function formatCleanupSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1).replace(".", ",")} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1).replace(".", ",")} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1).replace(".", ",")} GB`;
}

function formatCleanupDate(timestampMs: number): string {
  if (!Number.isFinite(timestampMs) || timestampMs <= 0) return "Data indisponível";
  return new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" }).format(timestampMs);
}

export type CleanupItem = {
  path: string;
  name: string;
  sizeBytes: number;
  isDir: boolean;
  modifiedAtUnixMs?: number | null;
};

export type CleanupCategory = {
  id: string;
  name: string;
  description: string;
  recommended: boolean;
  requiresAdmin?: boolean;
  availableBytes: number;
  itemCount: number;
  isPartial: boolean;
  unavailableReason?: string | null;
  largestItems: CleanupItem[];
};

export type GuidedInsight = {
  id: string;
  title: string;
  detail: string;
  path?: string | null;
  sizeBytes?: number | null;
  unavailableReason?: string | null;
  warning: string;
  command: string;
};

type CleanupAnalysis = { categories: CleanupCategory[]; scannedAtUnixMs: number };
type CleanupResult = { reclaimedBytes: number; deletedFiles: number; deletedDirectories: number; skippedItems: number };

/** Agrupamento visual por natureza do item; ids desconhecidos caem em "Outros". */
const CLEANUP_GROUPS: { id: string; title: string; hint: string; ids: string[] }[] = [
  { id: "system", title: "Sistema e temporários", hint: "Recriados pelo Windows e pelos aplicativos", ids: ["user-temp", "windows-temp", "delivery-optimization", "internet-cache"] },
  { id: "browser", title: "Navegadores", hint: "Sites recarregam o cache na próxima visita", ids: ["browser-cache"] },
  { id: "gpu", title: "Jogos e GPU", hint: "Shaders recompilados na primeira abertura", ids: ["shader-cache", "nvidia-shader-cache", "nvidia-installers"] },
  { id: "dev", title: "Desenvolvimento", hint: "Pacotes baixados de novo sob demanda", ids: ["dev-package-caches"] },
  { id: "review", title: "Revisar antes de limpar", hint: "Podem conter algo que você ainda quer", ids: ["crash-dumps", "recycle-bin"] },
];

function groupCategories(categories: CleanupCategory[]) {
  const known = new Set(CLEANUP_GROUPS.flatMap(group => group.ids));
  const groups = CLEANUP_GROUPS.map(group => ({
    ...group,
    categories: categories.filter(category => group.ids.includes(category.id)),
  }));
  const others = categories.filter(category => !known.has(category.id));
  if (others.length > 0) groups.push({ id: "other", title: "Outros", hint: "", ids: [], categories: others });
  return groups.filter(group => group.categories.length > 0);
}

function sumBytes(categories: CleanupCategory[]): number {
  return categories.reduce((sum, category) => sum + (Number.isFinite(category.availableBytes) ? category.availableBytes : 0), 0);
}

function recommendedIds(categories: CleanupCategory[]): Set<string> {
  return new Set(categories.filter(category => category.recommended && category.availableBytes > 0).map(category => category.id));
}

function GuidedInsightCard({ insight }: { insight: GuidedInsight }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(insight.command);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  };
  return (
    <article className="cleanup-insight panel">
      <div className="cleanup-insight-head">
        <span>
          <strong>{insight.title}</strong>
          {insight.path && <small title={insight.path}>{insight.path}</small>}
        </span>
        <span className="cleanup-insight-size">
          {insight.sizeBytes != null ? (
            <>
              <b>{formatCleanupSize(insight.sizeBytes)}</b>
              <small>tamanho do arquivo</small>
            </>
          ) : (
            <small>{insight.unavailableReason ?? "Indisponível"}</small>
          )}
        </span>
      </div>
      <p>{insight.detail}</p>
      <p className="cleanup-insight-warning">{insight.warning}</p>
      <details className="cleanup-largest">
        <summary>VER COMANDOS</summary>
        <pre className="cleanup-insight-command">{insight.command}</pre>
        <button type="button" className="disk-scan-btn cleanup-secondary-btn" onClick={() => void copy()}>
          {copied ? "COPIADO" : "COPIAR COMANDOS"}
        </button>
      </details>
    </article>
  );
}

export function DiskCleanup() {
  const standardUser = useIsStandardUser();
  const [analysis, setAnalysis] = useState<CleanupAnalysis>();
  const [insights, setInsights] = useState<GuidedInsight[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<CleanupResult>();

  const categories = useMemo(() => analysis?.categories ?? [], [analysis]);
  const groups = useMemo(() => groupCategories(categories), [categories]);
  const totalBytes = useMemo(() => sumBytes(categories), [categories]);
  const largestBytes = useMemo(() => Math.max(0, ...categories.map(category => category.availableBytes || 0)), [categories]);
  const selectedCategories = useMemo(() => categories.filter(category => selected.has(category.id)), [categories, selected]);
  const selectedBytes = useMemo(() => sumBytes(selectedCategories), [selectedCategories]);
  const selectedShare = totalBytes > 0 ? Math.min(100, (selectedBytes / totalBytes) * 100) : 0;

  const loadInsights = () => {
    invoke<GuidedInsight[]>("get_cleanup_insights")
      .then(next => setInsights(Array.isArray(next) ? next : []))
      .catch(() => setInsights([]));
  };

  const analyze = async () => {
    setLoading(true);
    setError("");
    setResult(undefined);
    loadInsights();
    try {
      const next = await invoke<CleanupAnalysis>("analyze_disk_cleanup");
      setAnalysis(next);
      setSelected(recommendedIds(next.categories));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  };

  const toggle = (id: string, disabled: boolean) => {
    if (disabled) return;
    setSelected(current => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const clean = async () => {
    setConfirming(false);
    setCleaning(true);
    setError("");
    try {
      const cleanup = await invoke<CleanupResult>("clean_disk_categories", { categoryIds: [...selected] });
      setResult(cleanup);
      const next = await invoke<CleanupAnalysis>("analyze_disk_cleanup");
      setAnalysis(next);
      setSelected(new Set());
    } catch (cause) {
      setError(String(cause));
    } finally {
      setCleaning(false);
    }
  };

  if (!analysis && !loading) {
    return (
      <div className="disk-cleanup-container">
        <section className="cleanup-welcome panel">
          <div>
            <span className="eyebrow">LIMPEZA LOCAL E CONTROLADA</span>
            <h3>Descubra espaço que pode ser recuperado.</h3>
            <p>O Pulse verifica somente caches, temporários e relatórios conhecidos. Nada é removido durante a análise.</p>
          </div>
          <button type="button" className="btn-primary disk-scan-btn cleanup-analyze-btn" onClick={() => void analyze()}>ANALISAR AGORA</button>
        </section>
        <AutoCleanupPanel />
      </div>
    );
  }

  const busy = loading || cleaning;

  return (
    <section className="disk-cleanup-container" aria-label="Limpeza de disco">
      <header className="cleanup-toolbar panel">
        <div className="cleanup-toolbar-row">
          <div className="cleanup-hero">
            <div className="cleanup-hero-stat">
              <span className="disk-toolbar-label">RECUPERÁVEL</span>
              <strong>{loading ? "Calculando…" : formatCleanupSize(totalBytes)}</strong>
              <small>{analysis ? `Analisado em ${formatCleanupDate(analysis.scannedAtUnixMs)}` : "Analisando categorias…"}</small>
            </div>
            <div className="cleanup-selection-summary">
              <span className="disk-toolbar-label">ESPAÇO SELECIONADO</span>
              <div>
                <strong>{loading ? "Calculando…" : formatCleanupSize(selectedBytes)}</strong>
                <small>{selected.size} {selected.size === 1 ? "categoria selecionada" : "categorias selecionadas"}</small>
              </div>
              <span className="cleanup-share-track" aria-hidden="true">
                <span style={{ width: `${selectedShare}%` }} />
              </span>
            </div>
          </div>
          <div className="cleanup-toolbar-actions">
            <button type="button" className="disk-scan-btn cleanup-ghost-btn" onClick={() => setSelected(recommendedIds(categories))} disabled={busy || categories.length === 0}>
              RECOMENDADAS
            </button>
            <button type="button" className="disk-scan-btn cleanup-ghost-btn" onClick={() => setSelected(new Set())} disabled={busy || selected.size === 0}>
              LIMPAR SELEÇÃO
            </button>
            <button type="button" className="disk-scan-btn cleanup-secondary-btn" onClick={() => void analyze()} disabled={busy}>
              {loading ? "ANALISANDO…" : "ANALISAR NOVAMENTE"}
            </button>
            <button type="button" className="btn-primary disk-scan-btn cleanup-clean-btn" onClick={() => setConfirming(true)} disabled={selected.size === 0 || busy}>
              {cleaning ? "LIMPANDO…" : "REVISAR E LIMPAR"}
            </button>
          </div>
        </div>
      </header>

      <AutoCleanupPanel />

      {error && <div className="cleanup-message error" role="alert">{error}</div>}
      {result && (
        <div className="cleanup-message success" role="status">
          {formatCleanupSize(result.reclaimedBytes)} liberados · {result.deletedFiles} arquivos removidos
          {result.skippedItems > 0 ? ` · ${result.skippedItems} itens em uso ou protegidos preservados` : ""}
        </div>
      )}

      <div className="cleanup-category-list">
        {groups.map(group => (
          <section className="cleanup-group" key={group.id} aria-label={group.title}>
            <header className="cleanup-group-head">
              <span className="disk-toolbar-label">{group.title.toUpperCase()}</span>
              {group.hint && <small>{group.hint}</small>}
              <b>{formatCleanupSize(sumBytes(group.categories))}</b>
            </header>
            <div className="cleanup-group-grid">
              {group.categories.map(category => {
                const empty = category.availableBytes <= 0;
                const share = largestBytes > 0 ? Math.max(empty ? 0 : 2, (category.availableBytes / largestBytes) * 100) : 0;
                return (
                  <article className={`cleanup-category panel ${selected.has(category.id) ? "selected" : ""} ${empty ? "is-empty" : ""}`} key={category.id}>
                    <label className="cleanup-category-choice">
                      <input
                        type="checkbox"
                        checked={selected.has(category.id)}
                        disabled={empty}
                        onChange={() => toggle(category.id, empty)}
                      />
                      <span className="cleanup-checkmark" aria-hidden="true" />
                      <span>
                        <strong>{category.name}</strong>
                        <small>{category.description}</small>
                      </span>
                    </label>
                    <div className="cleanup-category-size">
                      <strong>{formatCleanupSize(category.availableBytes)}</strong>
                      <small>{category.unavailableReason ?? `${category.itemCount} itens${category.isPartial ? " · estimativa parcial" : ""}`}</small>
                      {category.recommended ? (
                        <span className="cleanup-badge is-safe">RECRIADO AUTOMATICAMENTE</span>
                      ) : (
                        <span className="cleanup-badge is-review">REVISE ANTES</span>
                      )}
                      {standardUser && category.requiresAdmin === true && (
                        <span
                          className="cleanup-badge is-admin"
                          title="Pasta do sistema: sem administrador a análise fica parcial e itens protegidos são pulados. Use PERMISSÃO no topo para reiniciar como administrador."
                        >
                          EXIGE ADMINISTRADOR
                        </span>
                      )}
                    </div>
                    <span className="cleanup-size-track" aria-hidden="true">
                      <span style={{ width: `${share}%` }} />
                    </span>
                    {category.largestItems.length > 0 && (
                      <details className="cleanup-largest">
                        <summary>MAIORES ITENS NESTA CATEGORIA</summary>
                        <div>
                          {category.largestItems.slice(0, 5).map(item => (
                            <div className="cleanup-item" key={item.path} title={item.path}>
                              <span className={item.isDir ? "folder" : "file"} aria-hidden="true" />
                              <span><strong>{item.name}</strong><small>{item.modifiedAtUnixMs ? formatCleanupDate(item.modifiedAtUnixMs) : "Data indisponível"}</small></span>
                              <b>{formatCleanupSize(item.sizeBytes)}</b>
                            </div>
                          ))}
                        </div>
                      </details>
                    )}
                  </article>
                );
              })}
            </div>
          </section>
        ))}

        {insights.length > 0 && (
          <section className="cleanup-group" aria-label="Recuperação guiada">
            <header className="cleanup-group-head">
              <span className="disk-toolbar-label">RECUPERAÇÃO GUIADA · VOCÊ EXECUTA</span>
              <small>Exigem administrador ou fechar programas; o Pulse só detecta e explica</small>
            </header>
            <div className="cleanup-group-grid">
              {insights.map(insight => <GuidedInsightCard insight={insight} key={insight.id} />)}
            </div>
          </section>
        )}
      </div>

      {confirming && (
        <div className="cleanup-dialog-backdrop">
          <div className="cleanup-dialog panel" role="dialog" aria-modal="true" aria-labelledby="cleanup-confirm-title">
            <span className="eyebrow">CONFIRMAÇÃO OBRIGATÓRIA</span>
            <h3 id="cleanup-confirm-title">Liberar até {formatCleanupSize(selectedBytes)}?</h3>
            <p>Os itens das {selected.size} categorias marcadas serão removidos permanentemente. Arquivos bloqueados ou protegidos serão ignorados.</p>
            <ul className="cleanup-dialog-list">
              {selectedCategories.map(category => (
                <li key={category.id}>
                  <span>{category.name}</span>
                  <b>{formatCleanupSize(category.availableBytes)}</b>
                </li>
              ))}
            </ul>
            <div>
              <button type="button" className="btn-primary disk-scan-btn cleanup-clean-btn" onClick={() => void clean()}>CONFIRMAR LIMPEZA</button>
              <button type="button" className="disk-scan-btn cleanup-secondary-btn" onClick={() => setConfirming(false)}>CANCELAR</button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
