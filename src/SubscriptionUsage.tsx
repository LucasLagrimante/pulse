import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { DragEventHandler } from "react";
import { ModuleIcon } from "./Identity";
import { ProviderIcon } from "./ProviderIcon";
import { CREDITS_WINDOW_ID, QuotaHistoryButton } from "./QuotaHistory";

export type QuotaWindow = {
  id: string;
  label: string;
  usedPercent?: number | null;
  remainingPercent?: number | null;
  windowMinutes?: number | null;
  resetsAtUnixSeconds?: number | null;
  resetDescription?: string | null;
  unavailableReason?: string | null;
};

export type ProviderUsage = {
  id: "claude" | "codex" | "antigravity" | string;
  provider: string;
  subtitle: string;
  plan?: string | null;
  environment?: string | null;
  status: "available" | "unavailable" | string;
  windows: QuotaWindow[];
  creditsRemaining?: number | null;
  creditsTotal?: number | null;
  creditsUsed?: number | null;
  resetCreditsAvailable?: number | null;
  sampledAtUnixMs?: number | null;
  source: string;
  unavailableReason?: string | null;
  disabled?: boolean;
};

export type SubscriptionUsageReport = {
  providers: ProviderUsage[];
  sampledAtUnixMs: number;
  durationMs: number;
  privacyNote: string;
  scanStatus?: "running" | "ready" | "error" | string;
};

type SubscriptionSettings = {
  openrouterConfigured: boolean;
  openrouterMasked?: string | null;
  opencodeGoConfigured: boolean;
  opencodeGoMasked?: string | null;
  disabledProviders?: string[];
  providerOrder?: string[];
};

export function safePercent(value: number | null | undefined): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  return Math.max(0, Math.min(100, value));
}

export function measuredWindowMinutes(window: QuotaWindow): number | null {
  return typeof window.windowMinutes === "number" && Number.isFinite(window.windowMinutes) && window.windowMinutes > 0
    ? window.windowMinutes
    : null;
}

export function shortQuotaWindowLabel(window: QuotaWindow): string {
  const minutes = measuredWindowMinutes(window);
  if (minutes != null) {
    if (minutes % 1440 === 0) return `${Math.round(minutes / 1440)}d`;
    if (minutes % 60 === 0) return `${Math.round(minutes / 60)}h`;
    return `${Math.round(minutes)}min`;
  }
  const label = (window.label || "").toLowerCase();
  const id = (window.id || "").toLowerCase();
  if (label.includes("sess") || id.includes("sess") || label.includes("5h") || label.includes("5 hora")) return "5h";
  if (label.includes("sem") || id.includes("week") || label.includes("7d") || label.includes("7 dia")) return "7d";
  if (label.includes("mensal") || id.includes("month") || label.includes("30d")) return "30d";
  const fallback = window.label?.trim() || "Cota";
  return fallback.length > 6 ? `${fallback.slice(0, 4)}…` : fallback;
}

export function selectFlyoutQuotaWindows(windows: QuotaWindow[], limit = 2): QuotaWindow[] {
  const measured = (Array.isArray(windows) ? windows : []).filter(
    window => safePercent(window.remainingPercent) != null && !window.unavailableReason,
  );
  const byWindow = new Map<string, QuotaWindow>();
  for (const window of measured) {
    const minutes = measuredWindowMinutes(window);
    const key = minutes != null ? `m${minutes}` : `l${window.label || window.id || "cota"}`;
    const current = byWindow.get(key);
    const remaining = safePercent(window.remainingPercent) ?? 100;
    const currentRemaining = current ? safePercent(current.remainingPercent) ?? 100 : null;
    if (currentRemaining == null || remaining < currentRemaining) {
      byWindow.set(key, window);
    }
  }
  return [...byWindow.values()]
    .sort((a, b) => (measuredWindowMinutes(a) ?? Number.MAX_SAFE_INTEGER) - (measuredWindowMinutes(b) ?? Number.MAX_SAFE_INTEGER))
    .slice(0, limit);
}

export function formatReset(timestampSeconds: number | null | undefined, nowMs = Date.now()): string {
  if (typeof timestampSeconds !== "number" || !Number.isFinite(timestampSeconds)) return "Reset não informado";
  const difference = timestampSeconds * 1000 - nowMs;
  if (difference <= 0) return "Leitura vencida";
  const totalMinutes = Math.max(1, Math.ceil(difference / 60_000));
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return `Reseta em ${days}d ${hours}h`;
  if (hours > 0) return `Reseta em ${hours}h ${minutes}min`;
  return `Reseta em ${minutes}min`;
}

function planLabel(plan: string | null | undefined) {
  if (!plan?.trim()) return null;
  return plan.split("_").map(part => part.charAt(0).toLocaleUpperCase("pt-BR") + part.slice(1)).join(" ");
}

function measuredNumber(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

export function hasMeasuredQuota(provider: ProviderUsage): boolean {
  const windows = Array.isArray(provider.windows) ? provider.windows : [];
  const hasWindow = windows.some(window => safePercent(window.remainingPercent) != null && !window.unavailableReason);
  return provider.status === "available" && (hasWindow || measuredNumber(provider.creditsRemaining) != null);
}

export type KeyManagedProvider = "openrouter" | "opencode-go";

export function keyManagedProvider(provider: ProviderUsage): KeyManagedProvider | null {
  return provider.id === "openrouter" || provider.id === "opencode-go" ? provider.id : null;
}

export function isProviderDisabled(provider: ProviderUsage, disabledIds?: Set<string>): boolean {
  return provider.disabled === true || (disabledIds?.has(provider.id) ?? false);
}

export function formatUsd(value: number | null | undefined): string {
  const measured = measuredNumber(value);
  return measured == null
    ? "Indisponível"
    : `$ ${measured.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 4 })}`;
}

function formatHistoryCredits(providerId: string): (value: number) => string {
  if (providerId === "openrouter") return value => formatUsd(value);
  return value => `${value.toLocaleString("pt-BR", { maximumFractionDigits: 2 })} créditos`;
}

function PowerIcon() {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 3v9" />
      <path d="M6.6 6.6a8 8 0 1 0 10.8 0" />
    </svg>
  );
}

function KeyIcon() {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="7.5" cy="16" r="3.6" />
      <path d="M10.2 13.4 20 3.6" />
      <path d="M17 6.6l2 2" />
      <path d="M14.4 9.2l2 2" />
    </svg>
  );
}

function GripIcon() {
  return (
    <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor" aria-hidden="true">
      <circle cx="9" cy="6" r="1.5" />
      <circle cx="15" cy="6" r="1.5" />
      <circle cx="9" cy="12" r="1.5" />
      <circle cx="15" cy="12" r="1.5" />
      <circle cx="9" cy="18" r="1.5" />
      <circle cx="15" cy="18" r="1.5" />
    </svg>
  );
}

export function reorderIds(ids: string[], fromId: string, toId: string): string[] | null {
  const from = ids.indexOf(fromId);
  const to = ids.indexOf(toId);
  if (from < 0 || to < 0 || from === to) return null;
  const next = [...ids];
  next.splice(from, 1);
  next.splice(to, 0, fromId);
  return next;
}

function QuotaMeter({ quota, providerId, providerName }: { quota: QuotaWindow; providerId: string; providerName: string }) {
  const remaining = safePercent(quota.remainingPercent);
  const used = safePercent(quota.usedPercent);
  const available = remaining != null && !quota.unavailableReason;
  const reset = quota.resetDescription?.trim() || formatReset(quota.resetsAtUnixSeconds);
  const windowId = quota.id?.trim() || quota.label?.trim() || "cota";
  const windowLabel = quota.label?.trim() || "Janela sem nome";
  return (
    <div className={`quota-meter${available ? "" : " is-unavailable"}`}>
      <div className="quota-meter-heading">
        <span className="quota-meter-title">
          <strong>{windowLabel}</strong>
          {available && (
            <QuotaHistoryButton providerId={providerId} windowId={windowId} windowLabel={windowLabel} mode="percent" />
          )}
        </span>
        <span>{available ? reset : "Indisponível"}</span>
      </div>
      {available ? (
        <>
          <div
            className="quota-track"
            role="progressbar"
            aria-label={`${quota.label}: cota restante`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(remaining)}
          >
            <i className={remaining <= 20 ? "is-critical" : remaining <= 45 ? "is-warning" : ""} style={{ width: `${remaining}%` }} />
          </div>
          <div className="quota-meter-value">
            <b>{Math.round(remaining)}% restante</b>
            <small>{used == null ? "Uso indisponível" : `${Math.round(used)}% usado`}</small>
          </div>
        </>
      ) : <p>{quota.unavailableReason || `O ${providerName} não informou esta cota.`}</p>}
    </div>
  );
}

function ProviderCard({
  provider,
  disabled,
  onConfigureKey,
  onToggleEnabled,
  toggleBusy,
  reorderEnabled,
  dragging,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
}: {
  provider: ProviderUsage;
  disabled: boolean;
  onConfigureKey?: () => void;
  onToggleEnabled?: () => void;
  toggleBusy?: boolean;
  reorderEnabled?: boolean;
  dragging?: boolean;
  onDragStart?: DragEventHandler<HTMLElement>;
  onDragOver?: DragEventHandler<HTMLElement>;
  onDrop?: DragEventHandler<HTMLElement>;
  onDragEnd?: DragEventHandler<HTMLElement>;
}) {
  const windows = Array.isArray(provider.windows) ? provider.windows : [];
  const plan = planLabel(provider.plan);
  const credits = measuredNumber(provider.creditsRemaining);
  const creditsTotal = measuredNumber(provider.creditsTotal);
  const creditsUsed = measuredNumber(provider.creditsUsed);
  const available = hasMeasuredQuota(provider);
  const resetCredits = measuredNumber(provider.resetCreditsAvailable);
  const sampledAt = measuredNumber(provider.sampledAtUnixMs);
  const managedTarget = keyManagedProvider(provider);
  return (
    <article
      className={`quota-provider-card provider-${provider.id}${disabled ? " is-disabled" : available ? "" : " is-unavailable"}${reorderEnabled ? " is-reorderable" : ""}${dragging ? " is-dragging" : ""}`}
      draggable={reorderEnabled}
      onDragStart={onDragStart}
      onDragOver={onDragOver}
      onDrop={onDrop}
      onDragEnd={onDragEnd}
    >
      <header className="quota-provider-heading">
        {reorderEnabled && (
          <span className="quota-drag-handle" title="Arraste para reordenar" aria-hidden="true">
            <GripIcon />
          </span>
        )}
        <span className="quota-provider-mark" aria-hidden="true">
          <ProviderIcon providerId={provider.id} size={20} />
        </span>
        <div className="quota-provider-identity">
          <div className="quota-provider-title">
            <strong>{provider.provider || "Provedor desconhecido"}</strong>
            {plan && <span>{plan}</span>}
          </div>
          <small>{provider.subtitle || "Assinatura de IA"}</small>
        </div>
        <div className="quota-header-end">
          {onToggleEnabled && (
            <button
              type="button"
              className={`quota-icon-btn${disabled ? "" : " is-danger"}`}
              onClick={onToggleEnabled}
              disabled={toggleBusy}
              title={disabled ? `Reativar ${provider.provider}` : `Desativar ${provider.provider} (pausa as atualizações)`}
              aria-label={disabled ? `Reativar ${provider.provider}` : `Desativar ${provider.provider}`}
            >
              {toggleBusy ? <span className="quota-icon-busy" aria-hidden="true">…</span> : <PowerIcon />}
            </button>
          )}
          {!disabled && managedTarget && onConfigureKey && (
            <button
              type="button"
              className="quota-icon-btn"
              onClick={onConfigureKey}
              title={managedTarget === "openrouter" ? "Configurar chave de API do OpenRouter" : "Configurar chave de API do OpenCode Go"}
              aria-label={managedTarget === "openrouter" ? "Configurar chave OpenRouter" : "Configurar chave OpenCode Go"}
            >
              <KeyIcon />
            </button>
          )}
          <span className={`quota-status${disabled ? " is-disabled" : available ? " is-online" : ""}`}>
            {disabled ? "DESATIVADO" : available ? "ATUAL" : "INDISPONÍVEL"}
          </span>
        </div>
      </header>

      {disabled ? (
        <div className="quota-provider-empty quota-provider-disabled" role="status">
          <strong>Provedor desativado</strong>
          <p>As atualizações estão pausadas e este provedor não é exibido no ícone da bandeja. Reative para voltar a acompanhar a cota.</p>
        </div>
      ) : available ? (
        <div className="quota-window-list">
          {windows.length > 0 ? windows.map((quota, index) => <QuotaMeter key={quota.id || `${provider.id}-${index}`} quota={quota} providerId={provider.id} providerName={provider.provider} />) : (
            <div className="quota-credit-summary">
              <strong>{provider.id === "openrouter" ? "Saldo disponível" : "Créditos disponíveis"}</strong>
              <b>{credits != null ? (provider.id === "openrouter" ? formatUsd(credits) : credits.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 4 })) : "Indisponível"}</b>
              {credits != null && (
                <QuotaHistoryButton
                  providerId={provider.id}
                  windowId={CREDITS_WINDOW_ID}
                  windowLabel={provider.id === "openrouter" ? "Saldo (USD)" : "Créditos"}
                  mode="credits"
                  formatValue={formatHistoryCredits(provider.id)}
                />
              )}
            </div>
          )}
        </div>
      ) : (
        <div className="quota-provider-empty" role="status">
          <strong>Cota não confirmada</strong>
          <p>{provider.unavailableReason || "A fonte oficial não retornou percentuais."}</p>
          {managedTarget && onConfigureKey && (
            <button type="button" className="quota-card-setup-btn" onClick={onConfigureKey}>
              CONFIGURAR CHAVE {managedTarget === "openrouter" ? "OPENROUTER" : "OPENCODE GO"}
            </button>
          )}
        </div>
      )}

      {!disabled && (credits != null || creditsTotal != null || creditsUsed != null || resetCredits != null) && (
        <div className="quota-value-row">
          {credits != null && (
            <span>
              <small>{provider.id === "openrouter" ? "SALDO DISPONÍVEL (USD)" : "USO EXTRA"}</small>
              <strong>{provider.id === "openrouter" ? formatUsd(credits) : `${credits.toLocaleString("pt-BR", { maximumFractionDigits: 4 })} créditos`}</strong>
            </span>
          )}
          {creditsTotal != null && (
            <span>
              <small>{provider.id === "openrouter" ? "LIMITE TOTAL (USD)" : "LIMITE TOTAL"}</small>
              <strong>{provider.id === "openrouter" ? formatUsd(creditsTotal) : `${creditsTotal.toLocaleString("pt-BR", { maximumFractionDigits: 4 })} créditos`}</strong>
            </span>
          )}
          {creditsUsed != null && (
            <span>
              <small>{provider.id === "openrouter" ? "USO ACUMULADO (USD)" : "USO ACUMULADO"}</small>
              <strong>{provider.id === "openrouter" ? formatUsd(creditsUsed) : `${creditsUsed.toLocaleString("pt-BR", { maximumFractionDigits: 4 })} créditos`}</strong>
            </span>
          )}
          {resetCredits != null && <span><small>RESETS DE LIMITE</small><strong>{resetCredits.toLocaleString("pt-BR")} disponíveis</strong></span>}
        </div>
      )}

      <footer className="quota-provider-footer">
        <span>{provider.environment || "Ambiente não identificado"}</span>
        <span>{sampledAt == null ? "Sem horário confirmado" : new Date(sampledAt).toLocaleString("pt-BR")}</span>
        <span title={provider.source}>{provider.source || "Fonte não informada"}</span>
      </footer>
    </article>
  );
}

function QuotaLoading() {
  return (
    <div className="quota-loading-region">
      <div className="disk-panel-loading" role="status" aria-live="polite" aria-label="Consultando cotas locais">
        <span className="disk-panel-loading-mark"><ModuleIcon index={5} /></span>
        <div>
          <strong>CARREGANDO COTAS LOCAIS</strong>
          <span>Consultando fontes oficiais e credenciais locais…</span>
        </div>
        <i className="disk-panel-loading-line" />
      </div>
    </div>
  );
}

export function SubscriptionUsage() {
  const [report, setReport] = useState<SubscriptionUsageReport | null>(() => {
    try {
      const cached = localStorage.getItem("pulse-subscription-report");
      if (cached) {
        const parsed = JSON.parse(cached);
        if (parsed && Array.isArray(parsed.providers)) return parsed;
      }
    } catch {}
    return null;
  });
  const [loading, setLoading] = useState(() => !report);
  const [failure, setFailure] = useState<string | null>(null);
  const [settings, setSettings] = useState<SubscriptionSettings | null>(null);
  const [openrouterKey, setOpenrouterKey] = useState("");
  const [opencodeGoKey, setOpencodeGoKey] = useState("");
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [keyModalOpen, setKeyModalOpen] = useState(false);
  const [keyModalTarget, setKeyModalTarget] = useState<"openrouter" | "opencode-go">("openrouter");
  const [toggleBusy, setToggleBusy] = useState<string | null>(null);
  const [order, setOrder] = useState<string[]>([]);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [orderBusy, setOrderBusy] = useState(false);

  const load = useCallback(async (forceRefresh: boolean) => {
    setLoading(true);
    setFailure(null);
    try {
      let result: SubscriptionUsageReport | null = null;
      for (let attempt = 0; attempt < 18; attempt += 1) {
        const next = await invoke<SubscriptionUsageReport>("get_subscription_usage", { forceRefresh: forceRefresh && attempt === 0 });
        if (!next || !Array.isArray(next.providers)) throw new Error("Resposta local inválida");
        result = next;
        setReport(next);
        try {
          localStorage.setItem("pulse-subscription-report", JSON.stringify(next));
        } catch {}
        if (next.scanStatus !== "running") break;
        await new Promise(resolve => window.setTimeout(resolve, 700));
      }
      if (!result) throw new Error("Resposta local inválida");
    } catch {
      setFailure("O backend local não conseguiu consultar as assinaturas.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void load(false); }, [load]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<SubscriptionUsageReport>("pulse://subscription-usage-updated", event => {
      const payload = event.payload;
      if (payload && Array.isArray(payload.providers)) {
        setReport(payload);
        try {
          localStorage.setItem("pulse-subscription-report", JSON.stringify(payload));
        } catch {}
        if (payload.scanStatus !== "running") {
          setLoading(false);
        }
      }
    })
      .then(cleanup => {
        unlisten = cleanup;
      })
      .catch(() => undefined);

    const handleStorage = (event: StorageEvent) => {
      if (event.key === "pulse-subscription-report" && event.newValue) {
        try {
          const parsed = JSON.parse(event.newValue);
          if (parsed && Array.isArray(parsed.providers)) {
            setReport(parsed);
            setLoading(false);
          }
        } catch {}
      }
    };
    window.addEventListener("storage", handleStorage);

    return () => {
      if (unlisten) unlisten();
      window.removeEventListener("storage", handleStorage);
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void invoke<SubscriptionSettings>("get_subscription_settings").then(result => {
      if (!disposed && result && typeof result.openrouterConfigured === "boolean") setSettings(result);
    }).catch(() => undefined);
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    if (Array.isArray(settings?.providerOrder)) setOrder(settings.providerOrder);
  }, [settings?.providerOrder]);

  const openModal = (target: "openrouter" | "opencode-go") => {
    setSettingsError(null);
    if (target === "openrouter") setOpenrouterKey("");
    else setOpencodeGoKey("");
    setKeyModalTarget(target);
    setKeyModalOpen(true);
  };

  const saveKey = async () => {
    if (settingsBusy) return;
    if (keyModalTarget === "openrouter" && !openrouterKey.trim()) return;
    if (keyModalTarget === "opencode-go" && !opencodeGoKey.trim()) return;
    setSettingsBusy(true); setSettingsError(null);
    try {
      const command = keyModalTarget === "openrouter" ? "set_openrouter_api_key" : "set_opencode_go_api_key";
      const apiKey = keyModalTarget === "openrouter" ? openrouterKey : opencodeGoKey;
      const next = await invoke<SubscriptionSettings>(command, { apiKey });
      setSettings(next); setOpenrouterKey(""); setOpencodeGoKey(""); setKeyModalOpen(false); void load(true);
    } catch (error) {
      setSettingsError(typeof error === "string" ? error : "Não foi possível guardar a chave com segurança.");
    } finally { setSettingsBusy(false); }
  };

  const clearKey = async () => {
    if (settingsBusy) return;
    setSettingsBusy(true); setSettingsError(null);
    try {
      const command = keyModalTarget === "openrouter" ? "clear_openrouter_api_key" : "clear_opencode_go_api_key";
      setSettings(await invoke<SubscriptionSettings>(command)); void load(true);
    }
    catch (error) { setSettingsError(typeof error === "string" ? error : "Não foi possível remover a chave com segurança."); }
    finally { setSettingsBusy(false); }
  };

  const disabledIds = useMemo(
    () => new Set(settings?.disabledProviders ?? []),
    [settings],
  );

  const toggleProvider = async (provider: ProviderUsage, enabled: boolean) => {
    if (toggleBusy) return;
    setToggleBusy(provider.id);
    setSettingsError(null);
    try {
      const next = await invoke<SubscriptionSettings>("set_provider_enabled", { providerId: provider.id, enabled });
      if (next && Array.isArray(next.disabledProviders)) setSettings(next);
      setReport(previous => {
        if (!previous) return previous;
        const updated = {
          ...previous,
          providers: previous.providers.map(item => item.id === provider.id ? { ...item, disabled: !enabled } : item),
        };
        try { localStorage.setItem("pulse-subscription-report", JSON.stringify(updated)); } catch {}
        return updated;
      });
    } catch (error) {
      setSettingsError(typeof error === "string" ? error : "Não foi possível alterar o provedor.");
    } finally { setToggleBusy(null); }
  };

  const summary = useMemo(() => {
    const providers = report?.providers ?? [];
    const available = providers.filter(provider => !isProviderDisabled(provider, disabledIds) && hasMeasuredQuota(provider)).length;
    return { available, total: providers.length || 5 };
  }, [report, disabledIds]);
  const measuredProviders = useMemo(
    () => report?.providers.filter(hasMeasuredQuota) ?? [],
    [report],
  );

  const orderIndex = useMemo(() => {
    const map = new Map<string, number>();
    order.forEach((id, index) => map.set(id, index));
    return map;
  }, [order]);

  const displayProviders = useMemo(() => {
    const providers = report?.providers ?? [];
    const weight = (provider: ProviderUsage) => orderIndex.get(provider.id) ?? Number.MAX_SAFE_INTEGER;
    const visible = providers.filter(provider =>
      hasMeasuredQuota(provider) || keyManagedProvider(provider) != null || isProviderDisabled(provider, disabledIds),
    );
    const active = visible
      .filter(provider => !isProviderDisabled(provider, disabledIds))
      .sort((a, b) => weight(a) - weight(b));
    const inactive = visible
      .filter(provider => isProviderDisabled(provider, disabledIds))
      .sort((a, b) => weight(a) - weight(b));
    return [...active, ...inactive];
  }, [report, disabledIds, orderIndex]);

  const persistOrder = useCallback(async (nextOrder: string[]) => {
    setOrderBusy(true);
    setSettingsError(null);
    try {
      const next = await invoke<SubscriptionSettings>("set_provider_order", { order: nextOrder });
      if (next && Array.isArray(next.providerOrder)) setSettings(next);
    } catch (error) {
      setSettingsError(typeof error === "string" ? error : "Não foi possível salvar a ordem dos provedores.");
    } finally {
      setOrderBusy(false);
    }
  }, []);

  const handleDragStart = (id: string): DragEventHandler<HTMLElement> => event => {
    setDraggingId(id);
    event.dataTransfer.effectAllowed = "move";
    try { event.dataTransfer.setData("text/plain", id); } catch {}
  };
  const handleDragOver = (id: string): DragEventHandler<HTMLElement> => event => {
    if (!draggingId || draggingId === id) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
  };
  const handleDrop = (id: string): DragEventHandler<HTMLElement> => event => {
    event.preventDefault();
    const fromId = draggingId;
    setDraggingId(null);
    if (!fromId || fromId === id || orderBusy) return;
    const activeIds = displayProviders
      .filter(provider => !isProviderDisabled(provider, disabledIds))
      .map(provider => provider.id);
    const next = reorderIds(activeIds, fromId, id);
    if (!next) return;
    setOrder(next);
    void persistOrder(next);
  };
  const handleDragEnd = () => setDraggingId(null);


  const hasMeasuredProvider = measuredProviders.length > 0;
  const unavailableReasons = useMemo(
    () => report?.providers
      .filter(provider => !hasMeasuredQuota(provider) && provider.unavailableReason?.trim())
      .map(provider => `${provider.provider}: ${provider.unavailableReason}`)
      .join(" · "),
    [report],
  );

  return (
    <section className="module-dashboard quota-dashboard" aria-label="Cotas de assinatura de IA">
      <section className="panel quota-summary-panel">
        <div className="quota-toolbar">
          <div>
            <p className="eyebrow">ASSINATURAS DE IA</p>
            <h3>Limites e Cotas</h3>
            <p>Leitura local e oficial de cotas, tokens e créditos sem abrir terminais nem expor credenciais.</p>
          </div>
          <div className="quota-toolbar-actions">
            {!loading && report && <span><b>{summary.available}</b> de {summary.total} disponíveis</span>}
            <button
              type="button"
              className="quota-config-btn"
              onClick={() => openModal("openrouter")}
              title="Configurar chave de API local do OpenRouter"
            >
              {settings?.openrouterConfigured ? "CHAVE OPENROUTER ✓" : "CONFIGURAR OPENROUTER"}
            </button>
            <button
              type="button"
              className="quota-config-btn"
              onClick={() => openModal("opencode-go")}
              title="Configurar chave de API local do OpenCode Go"
            >
              {settings?.opencodeGoConfigured ? "CHAVE OPENCODE GO ✓" : "CONFIGURAR OPENCODE GO"}
            </button>
            <button type="button" onClick={() => void load(true)} disabled={loading}>
              {loading ? "CONSULTANDO…" : "ATUALIZAR COTAS"}
            </button>
          </div>
        </div>

        {failure && !report ? (
          <div className="quota-page-empty" role="alert">
            <strong>CONSULTA INDISPONÍVEL</strong>
            <p>{failure}</p>
            <button type="button" onClick={() => void load(true)}>TENTAR NOVAMENTE</button>
          </div>
        ) : loading && !hasMeasuredProvider ? <QuotaLoading /> : report ? (
          <>
            {loading && <div className="quota-refreshing" role="status"><i />Atualizando fontes oficiais em segundo plano…</div>}
            {displayProviders.length > 0 ? (
              <div className="quota-data-region">
                <div className="quota-provider-list">
                  {displayProviders.map(provider => {
                    const target = keyManagedProvider(provider);
                    const disabled = isProviderDisabled(provider, disabledIds);
                    return (
                      <ProviderCard
                        key={provider.id}
                        provider={provider}
                        disabled={disabled}
                        toggleBusy={toggleBusy === provider.id}
                        onToggleEnabled={() => void toggleProvider(provider, disabled)}
                        onConfigureKey={target ? () => openModal(target) : undefined}
                        reorderEnabled={!disabled && !orderBusy}
                        dragging={draggingId === provider.id}
                        onDragStart={handleDragStart(provider.id)}
                        onDragOver={handleDragOver(provider.id)}
                        onDrop={handleDrop(provider.id)}
                        onDragEnd={handleDragEnd}
                      />
                    );
                  })}
                </div>
              </div>
            ) : (
              <div className="quota-page-empty" role="status">
                <strong>NENHUMA COTA CONFIRMADA</strong>
                <p>As fontes locais não retornaram dados utilizáveis nesta consulta.</p>
                {unavailableReasons && <small>{unavailableReasons}</small>}
              </div>
            )}
          </>
        ) : null}

        <aside className="quota-privacy-note">
          <strong>Leitura privada e segura.</strong>
          <span>{report?.privacyNote || "O Pulse consulta as fontes locais e oficiais diretamente; tokens nunca são trafegados no DOM."}</span>
          {report && <small>{report.scanStatus === "running" ? "Leitura em andamento…" : `Consulta concluída em ${(report.durationMs / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })}s.`}</small>}
        </aside>
      </section>

      {/* Modal / Dialog para configuração de chaves externas sem quebrar o layout da tela */}
      {keyModalOpen && (
        <div className="quota-modal-overlay" role="dialog" aria-modal="true" aria-labelledby="modal-quota-key-title">
          <div className="quota-modal-card">
            <header className="quota-modal-header">
              <div>
                <p className="eyebrow">{keyModalTarget === "openrouter" ? "SALDO EM DÓLARES (USD)" : "COTA DA ASSINATURA"}</p>
                <h3 id="modal-quota-key-title">{keyModalTarget === "openrouter" ? "Configurar OpenRouter" : "Configurar OpenCode Go"}</h3>
              </div>
              <button
                type="button"
                className="quota-modal-close"
                onClick={() => setKeyModalOpen(false)}
                aria-label="Fechar modal"
              >
                ✕
              </button>
            </header>
            <div className="quota-modal-body">
              <p>A chave fica armazenada com criptografia no <strong>Credential Manager do Windows</strong>. O Pulse consulta apenas a cota oficial sem persistir o texto da chave na interface.</p>

              <div className="quota-key-form">
                <label>
                  <span>{keyModalTarget === "openrouter" ? "CHAVE OPENROUTER (SK-OR-…)" : "CHAVE OPENCODE GO (SK-…)"}</span>
                  <input
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    value={keyModalTarget === "openrouter" ? openrouterKey : opencodeGoKey}
                    onChange={event => keyModalTarget === "openrouter" ? setOpenrouterKey(event.target.value) : setOpencodeGoKey(event.target.value)}
                    placeholder={keyModalTarget === "openrouter" ? (settings?.openrouterMasked || "sk-or-…") : (settings?.opencodeGoMasked || "sk-…")}
                    aria-label={keyModalTarget === "openrouter" ? "Chave OpenRouter" : "Chave OpenCode Go"}
                  />
                </label>
                {keyModalTarget === "openrouter" && settings?.openrouterConfigured && (
                  <small className="quota-key-state">Chave atual salva: {settings.openrouterMasked}</small>
                )}
                {keyModalTarget === "opencode-go" && settings?.opencodeGoConfigured && (
                  <small className="quota-key-state">Chave atual salva: {settings.opencodeGoMasked}</small>
                )}
                {settingsError && <small className="quota-key-error" role="alert">{settingsError}</small>}

                <div className="quota-key-actions">
                  <button
                    type="button"
                    onClick={() => void saveKey()}
                    disabled={settingsBusy || (keyModalTarget === "openrouter" ? !openrouterKey.trim() : !opencodeGoKey.trim())}
                  >
                    {settingsBusy ? "GUARDANDO…" : "SALVAR NO WINDOWS"}
                  </button>
                  {((keyModalTarget === "openrouter" && settings?.openrouterConfigured) || (keyModalTarget === "opencode-go" && settings?.opencodeGoConfigured)) && (
                    <button
                      type="button"
                      className="is-danger"
                      onClick={() => void clearKey()}
                      disabled={settingsBusy}
                    >
                      REMOVER CHAVE
                    </button>
                  )}
                  <button
                    type="button"
                    className="is-secondary"
                    onClick={() => setKeyModalOpen(false)}
                  >
                    CANCELAR
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
