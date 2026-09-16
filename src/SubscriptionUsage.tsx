import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ModuleIcon } from "./Identity";
import { ProviderIcon } from "./ProviderIcon";

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
};

export function safePercent(value: number | null | undefined): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  return Math.max(0, Math.min(100, value));
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

export function formatUsd(value: number | null | undefined): string {
  const measured = measuredNumber(value);
  return measured == null
    ? "Indisponível"
    : `$ ${measured.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 4 })}`;
}

function QuotaMeter({ quota, providerId }: { quota: QuotaWindow; providerId: string }) {
  const remaining = safePercent(quota.remainingPercent);
  const used = safePercent(quota.usedPercent);
  const available = remaining != null && !quota.unavailableReason;
  const reset = quota.resetDescription?.trim() || formatReset(quota.resetsAtUnixSeconds);
  return (
    <div className={`quota-meter${available ? "" : " is-unavailable"}`}>
      <div className="quota-meter-heading">
        <strong>{quota.label || "Janela sem nome"}</strong>
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
      ) : <p>{quota.unavailableReason || `O ${providerId} não informou esta cota.`}</p>}
    </div>
  );
}

function ProviderCard({ provider, onConfigureKey }: { provider: ProviderUsage; onConfigureKey?: () => void }) {
  const windows = Array.isArray(provider.windows) ? provider.windows : [];
  const plan = planLabel(provider.plan);
  const credits = measuredNumber(provider.creditsRemaining);
  const creditsTotal = measuredNumber(provider.creditsTotal);
  const creditsUsed = measuredNumber(provider.creditsUsed);
  const available = hasMeasuredQuota(provider);
  const resetCredits = measuredNumber(provider.resetCreditsAvailable);
  const sampledAt = measuredNumber(provider.sampledAtUnixMs);
  return (
    <article className={`quota-provider-card provider-${provider.id}${available ? "" : " is-unavailable"}`}>
      <header className="quota-provider-heading">
        <span className="quota-provider-mark" aria-hidden="true">
          <ProviderIcon providerId={provider.id} size={20} />
        </span>
        <div>
          <div className="quota-provider-title">
            <strong>{provider.provider || "Provedor desconhecido"}</strong>
            {plan && <span>{plan}</span>}
          </div>
          <small>{provider.subtitle || "Assinatura de IA"}</small>
        </div>
        <div className="quota-header-end">
          {provider.id === "openrouter" && onConfigureKey && (
            <button
              type="button"
              className="quota-mini-action"
              onClick={onConfigureKey}
              title="Configurar chave de API do OpenRouter"
              aria-label="Configurar chave OpenRouter"
            >
              Chave
            </button>
          )}
          <span className={`quota-status ${available ? "is-online" : ""}`}>{available ? "ATUAL" : "INDISPONÍVEL"}</span>
        </div>
      </header>

      {available ? (
        <div className="quota-window-list">
          {windows.length > 0 ? windows.map((quota, index) => <QuotaMeter key={quota.id || `${provider.id}-${index}`} quota={quota} providerId={provider.provider} />) : (
            <div className="quota-credit-summary">
              <strong>{provider.id === "openrouter" ? "Saldo disponível" : "Créditos disponíveis"}</strong>
              <b>{credits != null ? (provider.id === "openrouter" ? formatUsd(credits) : credits.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 4 })) : "Indisponível"}</b>
            </div>
          )}
        </div>
      ) : (
        <div className="quota-provider-empty" role="status">
          <strong>Cota não confirmada</strong>
          <p>{provider.unavailableReason || "A fonte oficial não retornou percentuais."}</p>
          {provider.id === "openrouter" && onConfigureKey && (
            <button type="button" className="quota-card-setup-btn" onClick={onConfigureKey}>
              CONFIGURAR CHAVE OPENROUTER
            </button>
          )}
        </div>
      )}

      {(credits != null || creditsTotal != null || creditsUsed != null || resetCredits != null) && (
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
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [keyModalOpen, setKeyModalOpen] = useState(false);

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

  const saveOpenrouterKey = async () => {
    if (!openrouterKey.trim() || settingsBusy) return;
    setSettingsBusy(true); setSettingsError(null);
    try {
      const next = await invoke<SubscriptionSettings>("set_openrouter_api_key", { apiKey: openrouterKey });
      setSettings(next); setOpenrouterKey(""); setKeyModalOpen(false); void load(true);
    } catch (error) {
      setSettingsError(typeof error === "string" ? error : "Não foi possível guardar a chave com segurança.");
    } finally { setSettingsBusy(false); }
  };

  const clearOpenrouterKey = async () => {
    if (settingsBusy) return;
    setSettingsBusy(true); setSettingsError(null);
    try { setSettings(await invoke<SubscriptionSettings>("clear_openrouter_api_key")); void load(true); }
    catch (error) { setSettingsError(typeof error === "string" ? error : "Não foi possível remover a chave com segurança."); }
    finally { setSettingsBusy(false); }
  };

  const summary = useMemo(() => {
    const providers = report?.providers ?? [];
    const available = providers.filter(hasMeasuredQuota).length;
    return { available, total: providers.length || 4 };
  }, [report]);
  const measuredProviders = useMemo(
    () => report?.providers.filter(hasMeasuredQuota) ?? [],
    [report],
  );
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
              onClick={() => { setSettingsError(null); setKeyModalOpen(true); }}
              title="Configurar chaves de API locais (OpenRouter)"
            >
              {settings?.openrouterConfigured ? "CHAVE OPENROUTER ✓" : "CONFIGURAR OPENROUTER"}
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
            {measuredProviders.length > 0 ? (
              <div className="quota-data-region">
                <div className="quota-provider-list">
                  {measuredProviders.map(provider => (
                  <ProviderCard
                    key={provider.id}
                    provider={provider}
                    onConfigureKey={provider.id === "openrouter" ? () => { setSettingsError(null); setKeyModalOpen(true); } : undefined}
                  />
                  ))}
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
        <div className="quota-modal-overlay" role="dialog" aria-modal="true" aria-labelledby="modal-openrouter-title">
          <div className="quota-modal-card">
            <header className="quota-modal-header">
              <div>
                <p className="eyebrow">SALDO EM DÓLARES (USD)</p>
                <h3 id="modal-openrouter-title">Configurar OpenRouter</h3>
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
              <p>A chave fica armazenada com criptografia no <strong>Credential Manager do Windows</strong>. O Pulse consulta apenas o saldo em dólares e o uso acumulado sem persistir o texto da chave na interface.</p>

              <div className="quota-key-form">
                <label>
                  <span>CHAVE OPENROUTER (SK-OR-…)</span>
                  <input
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    value={openrouterKey}
                    onChange={event => setOpenrouterKey(event.target.value)}
                    placeholder={settings?.openrouterMasked || "sk-or-…"}
                    aria-label="Chave OpenRouter"
                  />
                </label>
                {settings?.openrouterConfigured && (
                  <small className="quota-key-state">Chave atual salva: {settings.openrouterMasked}</small>
                )}
                {settingsError && <small className="quota-key-error" role="alert">{settingsError}</small>}

                <div className="quota-key-actions">
                  <button
                    type="button"
                    onClick={() => void saveOpenrouterKey()}
                    disabled={settingsBusy || !openrouterKey.trim()}
                  >
                    {settingsBusy ? "GUARDANDO…" : "SALVAR NO WINDOWS"}
                  </button>
                  {settings?.openrouterConfigured && (
                    <button
                      type="button"
                      className="is-danger"
                      onClick={() => void clearOpenrouterKey()}
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
