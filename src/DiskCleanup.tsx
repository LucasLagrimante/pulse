import { invoke } from "@tauri-apps/api/core";
import { useMemo, useState } from "react";

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
  availableBytes: number;
  itemCount: number;
  isPartial: boolean;
  unavailableReason?: string | null;
  largestItems: CleanupItem[];
};

type CleanupAnalysis = { categories: CleanupCategory[]; scannedAtUnixMs: number };
type CleanupResult = { reclaimedBytes: number; deletedFiles: number; deletedDirectories: number; skippedItems: number };

export function DiskCleanup() {
  const [analysis, setAnalysis] = useState<CleanupAnalysis>();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<CleanupResult>();

  const selectedBytes = useMemo(
    () => analysis?.categories.filter(category => selected.has(category.id)).reduce((sum, category) => sum + category.availableBytes, 0) ?? 0,
    [analysis, selected],
  );

  const analyze = async () => {
    setLoading(true);
    setError("");
    setResult(undefined);
    try {
      const next = await invoke<CleanupAnalysis>("analyze_disk_cleanup");
      setAnalysis(next);
      setSelected(new Set(next.categories.filter(category => category.recommended && category.availableBytes > 0).map(category => category.id)));
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
      <section className="cleanup-welcome panel">
        <div>
          <span className="eyebrow">LIMPEZA LOCAL E CONTROLADA</span>
          <h3>Descubra espaço que pode ser recuperado.</h3>
          <p>O Pulse verifica somente caches, temporários e relatórios conhecidos. Nada é removido durante a análise.</p>
        </div>
        <button type="button" className="btn-primary disk-scan-btn cleanup-analyze-btn" onClick={() => void analyze()}>ANALISAR AGORA</button>
      </section>
    );
  }

  return (
    <section className="disk-cleanup-container" aria-label="Limpeza de disco">
      <header className="cleanup-toolbar panel">
        <div className="cleanup-toolbar-row">
          <div className="cleanup-selection-summary">
            <span className="disk-toolbar-label">ESPAÇO SELECIONADO</span>
            <div>
              <strong>{loading ? "Calculando…" : formatCleanupSize(selectedBytes)}</strong>
              <small>{selected.size} {selected.size === 1 ? "categoria selecionada" : "categorias selecionadas"}</small>
            </div>
          </div>
          <div className="cleanup-toolbar-actions">
            <button type="button" className="disk-scan-btn cleanup-secondary-btn" onClick={() => void analyze()} disabled={loading || cleaning}>
              {loading ? "ANALISANDO…" : "ANALISAR NOVAMENTE"}
            </button>
            <button type="button" className="btn-primary disk-scan-btn cleanup-clean-btn" onClick={() => setConfirming(true)} disabled={selected.size === 0 || loading || cleaning}>
              {cleaning ? "LIMPANDO…" : "REVISAR E LIMPAR"}
            </button>
          </div>
        </div>
      </header>

      {error && <div className="cleanup-message error" role="alert">{error}</div>}
      {result && (
        <div className="cleanup-message success" role="status">
          {formatCleanupSize(result.reclaimedBytes)} liberados · {result.deletedFiles} arquivos removidos
          {result.skippedItems > 0 ? ` · ${result.skippedItems} itens em uso ou protegidos preservados` : ""}
        </div>
      )}

      <div className="cleanup-category-list">
        {analysis?.categories.map(category => (
          <article className={`cleanup-category panel ${selected.has(category.id) ? "selected" : ""}`} key={category.id}>
            <label className="cleanup-category-choice">
              <input
                type="checkbox"
                checked={selected.has(category.id)}
                disabled={category.availableBytes <= 0}
                onChange={() => toggle(category.id, category.availableBytes <= 0)}
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
            </div>
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
        ))}
      </div>

      {confirming && (
        <div className="cleanup-dialog-backdrop">
          <div className="cleanup-dialog panel" role="dialog" aria-modal="true" aria-labelledby="cleanup-confirm-title">
            <span className="eyebrow">CONFIRMAÇÃO OBRIGATÓRIA</span>
            <h3 id="cleanup-confirm-title">Liberar até {formatCleanupSize(selectedBytes)}?</h3>
            <p>Os itens das {selected.size} categorias marcadas serão removidos permanentemente. Arquivos bloqueados ou protegidos serão ignorados.</p>
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
