import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";

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

const PROVIDER_MARKS: Record<string, string> = { claude: "C", codex: "◎", antigravity: "A" };

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

function ProviderCard({ provider }: { provider: ProviderUsage }) {
  const windows = Array.isArray(provider.windows) ? provider.windows : [];
  const plan = planLabel(provider.plan);
  const credits = measuredNumber(provider.creditsRemaining);
  const creditsTotal = measuredNumber(provider.creditsTotal);
  const creditsUsed = measuredNumber(provider.creditsUsed);
  const available = provider.status === "available" && (windows.length > 0 || credits != null);
  const resetCredits = measuredNumber(provider.resetCreditsAvailable);
  const sampledAt = measuredNumber(provider.sampledAtUnixMs);
  return (
    <article className={`quota-provider-card provider-${provider.id}${available ? "" : " is-unavailable"}`}>
      <header className="quota-provider-heading">
        <span className="quota-provider-mark" aria-hidden="true">{PROVIDER_MARKS[provider.id] || provider.provider?.charAt(0) || "?"}</span>
        <div>
          <div className="quota-provider-title">
            <strong>{provider.provider || "Provedor desconhecido"}</strong>
            {plan && <span>{plan}</span>}
          </div>
          <small>{provider.subtitle || "Assinatura de IA"}</small>
        </div>
        <span className={`quota-status ${available ? "is-online" : ""}`}>{available ? "ATUAL" : "INDISPONÍVEL"}</span>
      </header>

      {available ? (
        <div className="quota-window-list">
          {windows.length > 0 ? windows.map((quota, index) => <QuotaMeter key={quota.id || `${provider.id}-${index}`} quota={quota} providerId={provider.provider} />) : (
            <div className="quota-credit-summary"><strong>Créditos disponíveis</strong><b>{credits?.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 4 })}</b></div>
          )}
        </div>
      ) : (
        <div className="quota-provider-empty" role="status">
          <strong>Cota não confirmada</strong>
          <p>{provider.unavailableReason || "A fonte oficial não retornou percentuais."}</p>
        </div>
      )}

      {(credits != null || creditsTotal != null || creditsUsed != null || resetCredits != null) && (
        <div className="quota-value-row">
          {credits != null && <span><small>{provider.id === "openrouter" ? "CRÉDITOS DISPONÍVEIS" : "USO EXTRA"}</small><strong>{credits.toLocaleString("pt-BR", { maximumFractionDigits: 4 })} créditos</strong></span>}
          {creditsTotal != null && <span><small>LIMITE TOTAL</small><strong>{creditsTotal.toLocaleString("pt-BR", { maximumFractionDigits: 4 })} créditos</strong></span>}
          {creditsUsed != null && <span><small>USO ACUMULADO</small><strong>{creditsUsed.toLocaleString("pt-BR", { maximumFractionDigits: 4 })} créditos</strong></span>}
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

function LoadingCards() {
  return (
    <div className="quota-provider-list quota-skeleton-list" role="status" aria-label="Consultando fontes locais">
      {["Fonte local 01", "Fonte local 02", "Fonte local 03", "Fonte local 04"].map(provider => (
        <article className="quota-provider-card quota-skeleton" key={provider}>
          <header><i /><div><strong>{provider}</strong><span /></div></header>
          <div className="quota-skeleton-meter"><span /><i /></div>
          <div className="quota-skeleton-meter"><span /><i /></div>
        </article>
      ))}
    </div>
  );
}

export function SubscriptionUsage() {
  const [report, setReport] = useState<SubscriptionUsageReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [failure, setFailure] = useState<string | null>(null);
  const [settings, setSettings] = useState<SubscriptionSettings | null>(null);
  const [openrouterKey, setOpenrouterKey] = useState("");
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);

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
      setSettings(next); setOpenrouterKey(""); void load(true);
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
    const available = providers.filter(provider => provider.status === "available" && (provider.windows?.length > 0 || measuredNumber(provider.creditsRemaining) != null)).length;
    return { available, total: providers.length || 4 };
  }, [report]);
  const hasMeasuredProvider = report?.providers.some(provider => provider.status === "available" && (provider.windows?.length > 0 || measuredNumber(provider.creditsRemaining) != null)) ?? false;

  return (
    <section className="module-dashboard quota-dashboard" aria-label="Cotas de assinatura de IA">
      <section className="panel quota-summary-panel">
        <div className="quota-toolbar">
          <div>
            <p className="eyebrow">ASSINATURAS DE IA</p>
            <h3>Limites locais</h3>
            <p>Uma leitura única das fontes locais, sem abrir terminais nem expor credenciais.</p>
          </div>
          <div className="quota-toolbar-actions">
            {!loading && report && <span><b>{summary.available}</b> de {summary.total} disponíveis</span>}
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
        ) : loading && (!report || (report.scanStatus === "running" && !hasMeasuredProvider)) ? <LoadingCards /> : report ? (
          <>
            {loading && <div className="quota-refreshing" role="status"><i />Atualizando fontes locais em segundo plano…</div>}
            <div className="quota-provider-list">
              {report.providers
                .filter(provider => report.scanStatus !== "running" || provider.status === "available" && (provider.windows?.length > 0 || measuredNumber(provider.creditsRemaining) != null))
                .map(provider => <ProviderCard key={provider.id} provider={provider} />)}
            </div>
          </>
        ) : null}
      </section>

      <section className="panel quota-key-panel" aria-labelledby="openrouter-key-title">
        <div>
          <p className="eyebrow">CRÉDITOS DE API</p>
          <h3 id="openrouter-key-title">OpenRouter</h3>
          <p>A chave fica somente no Credential Manager do Windows. O Pulse recebe apenas o saldo e uma identificação mascarada.</p>
        </div>
        <div className="quota-key-form">
          <label>
            <span>CHAVE OPENROUTER</span>
            <input type="password" autoComplete="off" spellCheck={false} value={openrouterKey} onChange={event => setOpenrouterKey(event.target.value)} placeholder={settings?.openrouterMasked || "sk-or-…"} aria-label="Chave OpenRouter" />
          </label>
          <div className="quota-key-actions">
            <button type="button" onClick={() => void saveOpenrouterKey()} disabled={settingsBusy || !openrouterKey.trim()}>{settingsBusy ? "GUARDANDO…" : "SALVAR CHAVE"}</button>
            {settings?.openrouterConfigured && <button type="button" onClick={() => void clearOpenrouterKey()} disabled={settingsBusy}>REMOVER</button>}
          </div>
          {settings?.openrouterConfigured && <small className="quota-key-state">Configurada: {settings.openrouterMasked}</small>}
          {settingsError && <small className="quota-key-error" role="alert">{settingsError}</small>}
        </div>
      </section>

      <aside className="quota-privacy-note">
        <strong>Leitura privada.</strong>
        <span>{report?.privacyNote || "O Pulse abre os painéis oficiais localmente e nunca entrega credenciais à interface."}</span>
        {report && <small>{report.scanStatus === "running" ? "Leitura em andamento…" : `Consulta concluída em ${(report.durationMs / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })}s.`}</small>}
      </aside>
    </section>
  );
}
