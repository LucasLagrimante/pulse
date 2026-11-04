// Helpers puros de cotas de IA (sem JSX/React).
// Módulo leve propositalmente: o flyout do tray e o shell principal importam
// estes helpers sem puxar o componente pesado SubscriptionUsage (que fica em
// chunk lazy separado e só carrega quando a aba "Cotas de IA" abre).
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

export type KeyManagedProvider = "openrouter" | "opencode-go";

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

export function isWindowChartEligible(window: QuotaWindow): boolean {
  return safePercent(window.remainingPercent) != null && !window.unavailableReason;
}

export function formatReset(timestampSeconds: number | null | undefined, nowMs = Date.now()): string {
  if (typeof timestampSeconds !== "number" || !Number.isFinite(timestampSeconds)) return "Reset não informado";
  const difference = timestampSeconds * 1000 - nowMs;
  if (difference <= 0) return "Ciclo renovado";
  const totalMinutes = Math.max(1, Math.ceil(difference / 60_000));
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return `Reseta em ${days}d ${hours}h`;
  if (hours > 0) return `Reseta em ${hours}h ${minutes}min`;
  return `Reseta em ${minutes}min`;
}

export function planLabel(plan: string | null | undefined): string | null {
  if (!plan?.trim()) return null;
  return plan.split("_").map(part => part.charAt(0).toLocaleUpperCase("pt-BR") + part.slice(1)).join(" ");
}

export function measuredNumber(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

export function hasMeasuredQuota(provider: ProviderUsage): boolean {
  const windows = Array.isArray(provider.windows) ? provider.windows : [];
  const hasWindow = windows.some(window => safePercent(window.remainingPercent) != null && !window.unavailableReason);
  return provider.status === "available" && (hasWindow || measuredNumber(provider.creditsRemaining) != null);
}

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

export function reorderIds(ids: string[], fromId: string, toId: string): string[] | null {
  const from = ids.indexOf(fromId);
  const to = ids.indexOf(toId);
  if (from < 0 || to < 0 || from === to) return null;
  const next = [...ids];
  next.splice(from, 1);
  next.splice(to, 0, fromId);
  return next;
}

/** Contagem regressiva curta para hover/tooltip: `45min`, `2h30`, `4d2h`. */
export function formatCountdown(timestampSeconds: number | null | undefined, nowMs = Date.now()): string | null {
  if (typeof timestampSeconds !== "number" || !Number.isFinite(timestampSeconds)) return null;
  const difference = timestampSeconds * 1000 - nowMs;
  if (difference <= 0) return "renovado";
  const totalMinutes = Math.max(1, Math.ceil(difference / 60_000));
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return hours > 0 ? `${days}d${hours}h` : `${days}d`;
  if (hours > 0) return `${hours}h${String(minutes).padStart(2, "0")}`;
  return `${minutes}min`;
}

/** Nome de coluna por duração da janela (5 horas, Semanal, Mensal…). */
export function quotaColumnLabel(minutes: number | null): string {
  if (minutes === 300) return "5 horas";
  if (minutes === 10080) return "Semanal";
  if (minutes === 43200) return "Mensal";
  if (minutes == null) return "Janela";
  if (minutes % 1440 === 0) return `${minutes / 1440} dias`;
  if (minutes % 60 === 0) return `${minutes / 60} horas`;
  return `${minutes} min`;
}

/** "Gemini Models" → "Gemini"; "Claude and GPT models" → "Claude + GPT". */
export function shortModelName(model: string): string {
  const cleaned = model.replace(/\s+models?$/i, "").replace(/\s+and\s+/gi, " + ").trim();
  return cleaned || model.trim() || "Modelo";
}

export type QuotaMatrixRow = { model: string; cells: Map<number | null, QuotaWindow> };
export type QuotaMatrix = { columns: Array<number | null>; rows: QuotaMatrixRow[] };

/**
 * Agrupa janelas no formato "Modelo · Janela" em matriz modelo × duração.
 * Retorna null quando o provedor não separa cota por modelo (Codex, Claude),
 * que continuam na lista simples de barras.
 */
export function groupQuotaWindowsByModel(windows: QuotaWindow[]): QuotaMatrix | null {
  const list = Array.isArray(windows) ? windows : [];
  if (list.length < 3) return null;
  const rows = new Map<string, QuotaMatrixRow>();
  const columns = new Set<number | null>();
  for (const window of list) {
    const separator = (window.label || "").lastIndexOf(" · ");
    if (separator <= 0) return null;
    const model = window.label.slice(0, separator).trim();
    const minutes = measuredWindowMinutes(window);
    columns.add(minutes);
    const row = rows.get(model) ?? { model, cells: new Map() };
    // Janela duplicada para o mesmo modelo/duração: mantém a mais crítica.
    const current = row.cells.get(minutes);
    if (!current || (safePercent(window.remainingPercent) ?? 100) < (safePercent(current.remainingPercent) ?? 100)) {
      row.cells.set(minutes, window);
    }
    rows.set(model, row);
  }
  if (rows.size < 2) return null;
  return {
    columns: [...columns].sort((a, b) => (a ?? Number.MAX_SAFE_INTEGER) - (b ?? Number.MAX_SAFE_INTEGER)),
    rows: [...rows.values()],
  };
}
