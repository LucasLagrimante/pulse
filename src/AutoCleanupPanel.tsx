import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

type CleanupResult = { reclaimedBytes: number; deletedFiles: number; deletedDirectories: number; skippedItems: number };

export type AutoCleanupStatus = {
  enabled: boolean;
  intervalDays: number;
  categoryIds: string[];
  lastRunAtUnixMs?: number | null;
  lastResult?: CleanupResult | null;
  nextRunAtUnixMs?: number | null;
  minAgeDays: number;
  availableCategories: { id: string; name: string; recommended: boolean }[];
};

const AUTO_CLEANUP_EVENT = "pulse://auto-cleanup-finished";

const INTERVALS = [
  { days: 1, label: "Diária" },
  { days: 7, label: "Semanal" },
  { days: 30, label: "Mensal" },
];

function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1).replace(".", ",")} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1).replace(".", ",")} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1).replace(".", ",")} GB`;
}

function formatWhen(timestampMs: number | null | undefined): string | null {
  if (timestampMs == null || !Number.isFinite(timestampMs) || timestampMs <= 0) return null;
  return new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" }).format(timestampMs);
}

/** Resume o estado em uma linha: desligado, aguardando a 1ª execução ou último resultado. */
export function describeAutoCleanup(status: AutoCleanupStatus, nowMs = Date.now()): string {
  if (!status.enabled) return "Desativada — nada é removido sem a sua confirmação.";
  const last = formatWhen(status.lastRunAtUnixMs);
  const next = status.nextRunAtUnixMs != null && status.nextRunAtUnixMs > nowMs ? formatWhen(status.nextRunAtUnixMs) : null;
  const parts = [
    last && status.lastResult ? `Última: ${last} · ${formatBytes(status.lastResult.reclaimedBytes)} liberados` : "Ainda não executada",
    next ? `Próxima: ${next}` : "Próxima: assim que o PC ficar ocioso",
  ];
  return parts.join(" · ");
}

export function AutoCleanupPanel() {
  const [status, setStatus] = useState<AutoCleanupStatus>();
  const [expanded, setExpanded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    invoke<AutoCleanupStatus>("get_auto_cleanup")
      .then(next => {
        if (active && next) setStatus(next);
      })
      .catch(cause => {
        if (active) setError(String(cause));
      });
    const unlisten = listen<AutoCleanupStatus>(AUTO_CLEANUP_EVENT, event => {
      if (active && event.payload) setStatus(event.payload);
    });
    return () => {
      active = false;
      void unlisten.then(stop => stop());
    };
  }, []);

  if (!status || !Array.isArray(status.availableCategories)) return null;

  const save = async (patch: Partial<Pick<AutoCleanupStatus, "enabled" | "intervalDays" | "categoryIds">>) => {
    const next = { ...status, ...patch };
    setSaving(true);
    setError("");
    try {
      const saved = await invoke<AutoCleanupStatus>("set_auto_cleanup", {
        enabled: next.enabled,
        intervalDays: next.intervalDays,
        categoryIds: next.categoryIds,
      });
      setStatus(saved);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSaving(false);
    }
  };

  const toggleCategory = (id: string) => {
    const categoryIds = status.categoryIds.includes(id)
      ? status.categoryIds.filter(current => current !== id)
      : [...status.categoryIds, id];
    void save({ categoryIds });
  };

  return (
    <section className="auto-cleanup panel" aria-label="Limpeza automática">
      <div className="auto-cleanup-row">
        <div className="auto-cleanup-copy">
          <span className="disk-toolbar-label">LIMPEZA AUTOMÁTICA · OPCIONAL</span>
          <small>{describeAutoCleanup(status)}</small>
        </div>
        <div className="auto-cleanup-actions">
          <button
            type="button"
            className="disk-scan-btn cleanup-secondary-btn"
            aria-expanded={expanded}
            onClick={() => setExpanded(current => !current)}
          >
            {expanded ? "FECHAR" : "CONFIGURAR"}
          </button>
          <label className="debug-switch">
            <span>{status.enabled ? "Ativada" : "Desativada"}</span>
            <input
              aria-label="Ativar limpeza automática"
              type="checkbox"
              checked={status.enabled}
              disabled={saving}
              onChange={event => void save({ enabled: event.target.checked })}
            />
          </label>
        </div>
      </div>

      {error && <div className="cleanup-message error" role="alert">{error}</div>}

      {expanded && (
        <div className="auto-cleanup-settings">
          <p>
            Roda em segundo plano só com o PC ocioso há 5 minutos e na tomada. Arquivos modificados nos últimos{" "}
            {status.minAgeDays} dias são sempre preservados. A Lixeira e os caches de desenvolvimento ficam só na
            limpeza manual.
          </p>
          <div className="auto-cleanup-intervals" role="radiogroup" aria-label="Frequência da limpeza automática">
            {INTERVALS.map(interval => (
              <button
                type="button"
                role="radio"
                key={interval.days}
                aria-checked={status.intervalDays === interval.days}
                className={`auto-cleanup-chip ${status.intervalDays === interval.days ? "is-active" : ""}`}
                disabled={saving}
                onClick={() => void save({ intervalDays: interval.days })}
              >
                {interval.label}
              </button>
            ))}
          </div>
          <div className="auto-cleanup-categories">
            {status.availableCategories.map(category => (
              <label className="cleanup-category-choice auto-cleanup-category" key={category.id}>
                <input
                  type="checkbox"
                  aria-label={`Incluir na limpeza automática: ${category.name}`}
                  checked={status.categoryIds.includes(category.id)}
                  disabled={saving}
                  onChange={() => toggleCategory(category.id)}
                />
                <span className="cleanup-checkmark" aria-hidden="true" />
                <span>
                  <strong>{category.name}</strong>
                  {category.recommended && <small>Recomendada</small>}
                </span>
              </label>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
