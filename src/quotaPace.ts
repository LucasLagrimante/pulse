// Cálculo puro de ritmo de consumo (burn rate) e projeção de autonomia de cotas de IA.
// Sem dependências de React/Tauri; zero dados fictícios quando não há base suficiente.

import {
  formatCountdown,
  formatReset,
  measuredWindowMinutes,
  safePercent,
  type QuotaWindow,
} from "./subscriptionHelpers";

export type PaceRateUnit = "hour" | "day";

export type QuotaPaceStatus =
  | "expired"
  | "exhausted"
  | "will-deplete"
  | "tight"
  | "ok"
  | "idle"
  | "insufficient"
  | "no-reset";

export type QuotaPaceEstimate = {
  status: QuotaPaceStatus;
  /** Taxa de consumo em pontos percentuais por unidade (`hour` ou `day`). */
  ratePerUnit: number | null;
  rateUnit: PaceRateUnit;
  /** Percentual consumido projetado no instante do reset (pode ultrapassar 100%). */
  projectedEndPercent: number | null;
  /** Sobra estimada no reset em pontos percentuais (0 quando esgota antes). */
  projectedRemainingPercent: number | null;
  /** Milissegundos estimados até atingir 100% no ritmo atual. */
  timeToDepleteMs: number | null;
  /** Timestamp (ms) estimado de esgotamento quando `status === "will-deplete"`. */
  depleteAtUnixMs: number | null;
  /** Milissegundos restantes até o reset oficial. */
  remainingMs: number | null;
  /** Milissegundos já decorridos no ciclo atual. */
  elapsedMs: number | null;
  /** Início calculado do ciclo atual (ms). */
  cycleStartMs: number | null;
  /** Timestamp oficial de reset (ms). */
  resetsAtMs: number | null;
  /** Folga/antecipação em ms: negativo = esgota antes do reset; positivo = sobra tempo. */
  marginMs: number | null;
  /** Percentual usado atual (0..100). */
  usedPercent: number | null;
  /** Percentual restante atual (0..100). */
  remainingPercent: number | null;
};

export type TimedSample = { at: number; value: number };

export type RecentHistoryPace = {
  ratePerUnit: number;
  rateUnit: PaceRateUnit;
  spanMs: number;
  /** Comparação relativa ao ritmo do ciclo: positivo = acelerou, negativo = desacelerou. */
  trend: "faster" | "slower" | "steady";
  /** Texto curto do intervalo analisado, ex.: "últ. 3d", "últ. 2h" ou "últ. 45min". */
  windowLabel: string;
};

export type RecentPaceProjection = {
  /** Percentual consumido projetado no reset mantendo o ritmo recente. */
  projectedEndPercent: number;
  /** Sobra estimada no reset mantendo o ritmo recente (0 quando esgota antes). */
  projectedRemainingPercent: number;
  /** Milissegundos estimados até atingir 100% no ritmo recente (null se taxa === 0). */
  timeToDepleteMs: number | null;
  /** Timestamp (ms) estimado de esgotamento no ritmo recente quando `willDeplete === true`. */
  depleteAtUnixMs: number | null;
  /** Verdadeiro quando, no passo recente, a cota esgota antes do reset oficial. */
  willDeplete: boolean;
  /** Verdadeiro quando, no passo recente, chega no limite (sobra <= 15%) sem esgotar antes. */
  isTight: boolean;
};

export type MeterPaceRecentBadge = {
  trend: "faster" | "slower";
  tone: "ok" | "warning" | "danger";
  /** Texto compacto exibido na linha de ritmo da barra. */
  text: string;
  /** Tooltip comparando a média do ciclo com o passo recente e sua projeção. */
  tooltip: string;
  /** Verdadeiro quando o passo recente esgotaria a cota antes do reset. */
  depletesAtRecentPace: boolean;
};

export type CreditsBurnEstimate = {
  ratePerDay: number;
  /** Dias estimados de autonomia para o saldo atual, ou null se saldo for 0. */
  daysRemaining: number | null;
  spanDays: number;
};

export type PaceWindowInput = Pick<
  QuotaWindow,
  "id" | "label" | "usedPercent" | "remainingPercent" | "windowMinutes" | "resetsAtUnixSeconds" | "resetDescription"
>;

const HOUR_MS = 3_600_000;
const DAY_MS = 86_400_000;
/** Projeção a partir da qual o ritmo é considerado "no limite" (sobra <= 15%). */
const TIGHT_THRESHOLD_PERCENT = 85;

/** Resolve a duração da janela em minutos a partir do campo explícito ou do rótulo/id. */
export function resolveWindowMinutes(quota?: PaceWindowInput | null): number | null {
  if (!quota) return null;
  const explicit = measuredWindowMinutes(quota as QuotaWindow);
  if (explicit != null) return explicit;
  const label = (quota.label || "").toLowerCase();
  const id = (quota.id || "").toLowerCase();
  if (label.includes("5h") || label.includes("5 hora") || label.includes("sess") || id.includes("sess")) {
    return 300;
  }
  if (label.includes("sem") || id.includes("week") || label.includes("7d") || label.includes("7 dia")) {
    return 10080;
  }
  if (label.includes("mens") || id.includes("month") || label.includes("30d")) {
    return 43200;
  }
  return null;
}

/** Piso mínimo de tempo decorrido para evitar extrapolações ruidosas no início do ciclo. */
export function minimumElapsedMsForWindow(durationMs: number): number {
  if (durationMs <= 6 * HOUR_MS) return 15 * 60_000;
  if (durationMs <= 8 * DAY_MS) return 4 * HOUR_MS;
  if (durationMs <= 35 * DAY_MS) return 12 * HOUR_MS;
  return Math.max(15 * 60_000, Math.round(durationMs * 0.02));
}

export function paceUnitForWindow(durationMs: number): PaceRateUnit {
  return durationMs <= DAY_MS ? "hour" : "day";
}

function resolveUsedAndRemaining(quota: PaceWindowInput): { used: number | null; remaining: number | null } {
  const remaining = safePercent(quota.remainingPercent);
  const used = safePercent(quota.usedPercent);
  if (used != null && remaining != null) return { used, remaining };
  if (used != null) return { used, remaining: Math.max(0, 100 - used) };
  if (remaining != null) return { used: Math.max(0, 100 - remaining), remaining };
  return { used: null, remaining: null };
}

/**
 * Calcula velocidade de consumo e projeção de autonomia até o reset da janela.
 * Nunca inventa estimativa sem timestamp de reset ou antes do piso mínimo do ciclo.
 */
export function computeQuotaPace(
  quota: PaceWindowInput | null | undefined,
  nowMs = Date.now(),
): QuotaPaceEstimate | null {
  if (!quota) return null;
  const { used, remaining } = resolveUsedAndRemaining(quota);
  if (used == null || remaining == null) return null;

  const minutes = resolveWindowMinutes(quota);
  const durationMs = minutes != null ? minutes * 60_000 : null;
  const rateUnit: PaceRateUnit = durationMs != null ? paceUnitForWindow(durationMs) : "day";
  const resetsAtSeconds =
    typeof quota.resetsAtUnixSeconds === "number" &&
    Number.isFinite(quota.resetsAtUnixSeconds) &&
    quota.resetsAtUnixSeconds > 0
      ? quota.resetsAtUnixSeconds
      : null;
  const resetsAtMs = resetsAtSeconds != null ? resetsAtSeconds * 1000 : null;

  const base = {
    ratePerUnit: null,
    rateUnit,
    projectedEndPercent: null,
    projectedRemainingPercent: null,
    timeToDepleteMs: null,
    depleteAtUnixMs: null,
    remainingMs: resetsAtMs != null ? Math.max(0, resetsAtMs - nowMs) : null,
    elapsedMs: null,
    cycleStartMs: null,
    resetsAtMs,
    marginMs: null,
    usedPercent: used,
    remainingPercent: remaining,
  };

  if (resetsAtMs != null && resetsAtMs <= nowMs) {
    return { ...base, status: "expired", usedPercent: 0, remainingPercent: 100, remainingMs: 0 };
  }

  // Cota esgotada é um fato medido: independe de ter tempo decorrido mínimo ou timestamp.
  if (remaining <= 0 || used >= 100) {
    const cycleStartMs = resetsAtMs != null && durationMs != null ? Math.max(0, resetsAtMs - durationMs) : null;
    const elapsedMs = cycleStartMs != null ? Math.max(0, nowMs - cycleStartMs) : null;
    return {
      ...base,
      status: "exhausted",
      usedPercent: 100,
      remainingPercent: 0,
      cycleStartMs,
      elapsedMs,
      projectedEndPercent: 100,
      projectedRemainingPercent: 0,
      timeToDepleteMs: 0,
    };
  }

  if (resetsAtMs == null || durationMs == null || durationMs <= 0) {
    return { ...base, status: used <= 0 ? "idle" : "no-reset" };
  }

  const remainingMs = resetsAtMs - nowMs;
  const cycleStartMs = Math.max(0, resetsAtMs - durationMs);
  // Meses de 31 dias podem ter remainingMs ligeiramente maior que 30d (43200 min): clamp em 0.
  const elapsedMs = Math.max(0, nowMs - cycleStartMs);

  if (used <= 0) {
    return {
      ...base,
      status: "idle",
      cycleStartMs,
      elapsedMs,
      remainingMs,
    };
  }

  const minElapsed = minimumElapsedMsForWindow(durationMs);
  if (elapsedMs < minElapsed) {
    return {
      ...base,
      status: "insufficient",
      cycleStartMs,
      elapsedMs,
      remainingMs,
    };
  }

  const unitMs = rateUnit === "hour" ? HOUR_MS : DAY_MS;
  const ratePerMs = used / elapsedMs;
  const ratePerUnit = ratePerMs * unitMs;
  const projectedEndPercent = used + ratePerMs * remainingMs;
  const projectedRemainingPercent = Math.max(0, 100 - projectedEndPercent);
  const timeToDepleteMs = remaining / ratePerMs;
  const marginMs = timeToDepleteMs - remainingMs;
  const willDeplete = projectedEndPercent > 100.5 && timeToDepleteMs < remainingMs;

  const status: QuotaPaceStatus = willDeplete
    ? "will-deplete"
    : projectedEndPercent >= TIGHT_THRESHOLD_PERCENT
      ? "tight"
      : "ok";

  return {
    status,
    ratePerUnit,
    rateUnit,
    projectedEndPercent,
    projectedRemainingPercent,
    timeToDepleteMs,
    depleteAtUnixMs: willDeplete ? nowMs + timeToDepleteMs : null,
    remainingMs,
    elapsedMs,
    cycleStartMs,
    resetsAtMs,
    marginMs,
    usedPercent: used,
    remainingPercent: remaining,
  };
}

/**
 * Calcula o ritmo recente com base nas amostras históricas do próprio ciclo.
 * Compara o passo recente frente à média do ciclo, ignorando amostras obsoletas
 * e resets de ciclo para evitar extrapolações fictícias.
 */
export function computeRecentHistoryPace(
  samples: TimedSample[],
  cycleRatePerUnit: number | null,
  rateUnit: PaceRateUnit,
  nowMs = Date.now(),
  cycleStartMs?: number | null,
): RecentHistoryPace | null {
  if (samples.length < 2) return null;
  const minCycleAt =
    typeof cycleStartMs === "number" && Number.isFinite(cycleStartMs) ? cycleStartMs : null;
  const sorted = samples
    .filter(
      entry =>
        Number.isFinite(entry.at) &&
        Number.isFinite(entry.value) &&
        (minCycleAt == null || entry.at >= minCycleAt),
    )
    .sort((a, b) => a.at - b.at);
  if (sorted.length < 2) return null;

  const latest = sorted[sorted.length - 1];
  // Janela de recência proporcional à unidade: 2h para janelas curtas, 3d para semanais/mensais.
  const lookbackMs = rateUnit === "hour" ? 2 * HOUR_MS : 3 * DAY_MS;
  const minSpanMs = rateUnit === "hour" ? 30 * 60_000 : 18 * HOUR_MS;
  // Se a amostra mais recente já envelheceu mais que a própria janela de recência, não é "recente".
  if (nowMs - latest.at > lookbackMs) return null;

  const cutoff = Math.min(latest.at, nowMs) - lookbackMs;
  const recentWindow = sorted.filter(entry => entry.at >= cutoff);
  if (recentWindow.length < 2) return null;

  // Se houver queda brusca de reset dentro da janela recente, mede apenas o segmento pós-reset.
  let segmentStart = 0;
  for (let i = 1; i < recentWindow.length; i += 1) {
    if (recentWindow[i].value - recentWindow[i - 1].value <= -15) {
      segmentStart = i;
    }
  }
  const recent = recentWindow.slice(segmentStart);
  if (recent.length < 2) return null;

  const spanMs = recent[recent.length - 1].at - recent[0].at;
  if (spanMs < minSpanMs) return null;

  let positiveDelta = 0;
  let hadDrop = false;
  for (let i = 1; i < recent.length; i += 1) {
    const diff = recent[i].value - recent[i - 1].value;
    if (diff > 0) positiveDelta += diff;
    else if (diff < -1) hadDrop = true;
  }

  const unitMs = rateUnit === "hour" ? HOUR_MS : DAY_MS;
  let ratePerUnit: number;
  if (positiveDelta <= 0) {
    // Uso genuinamente estável (0%/unid.) após consumo anterior no ciclo: desaceleração real.
    const isMeasuredPause =
      !hadDrop &&
      cycleRatePerUnit != null &&
      cycleRatePerUnit > 0 &&
      recent[0].value > 0 &&
      recent[recent.length - 1].value >= recent[0].value;
    if (!isMeasuredPause) return null;
    ratePerUnit = 0;
  } else {
    ratePerUnit = (positiveDelta / spanMs) * unitMs;
  }

  let trend: RecentHistoryPace["trend"] = "steady";
  if (cycleRatePerUnit != null && cycleRatePerUnit > 0) {
    const ratio = ratePerUnit / cycleRatePerUnit;
    if (ratio >= 1.35) trend = "faster";
    else if (ratio <= 0.65) trend = "slower";
  }

  const windowLabel =
    rateUnit === "hour"
      ? spanMs < 55 * 60_000
        ? `últ. ${Math.max(1, Math.round(spanMs / 60_000))}min`
        : `últ. ${Math.max(1, Math.round(spanMs / HOUR_MS))}h`
      : spanMs < 22 * HOUR_MS
        ? `últ. ${Math.max(1, Math.round(spanMs / HOUR_MS))}h`
        : `últ. ${Math.max(1, Math.round(spanMs / DAY_MS))}d`;

  return { ratePerUnit, rateUnit, spanMs, trend, windowLabel };
}

/**
 * Projeta o fechamento do ciclo mantendo o ritmo recente medido no histórico.
 * Permite detectar risco de esgotamento antes do reset mesmo quando a média do ciclo parece folgada.
 */
export function projectPaceAtRecentRate(
  pace: QuotaPaceEstimate,
  recent: RecentHistoryPace,
  nowMs = Date.now(),
): RecentPaceProjection | null {
  if (
    pace.remainingPercent == null ||
    pace.usedPercent == null ||
    pace.remainingMs == null ||
    pace.remainingMs <= 0 ||
    pace.status === "exhausted" ||
    pace.status === "expired"
  ) {
    return null;
  }
  if (!Number.isFinite(recent.ratePerUnit) || recent.ratePerUnit < 0) return null;
  const unitMs = recent.rateUnit === "hour" ? HOUR_MS : DAY_MS;
  const ratePerMs = recent.ratePerUnit / unitMs;
  const projectedEndPercent = pace.usedPercent + ratePerMs * pace.remainingMs;
  const projectedRemainingPercent = Math.max(0, 100 - projectedEndPercent);
  const timeToDepleteMs = ratePerMs > 0 ? pace.remainingPercent / ratePerMs : null;
  const willDeplete =
    timeToDepleteMs != null &&
    projectedEndPercent > 100.5 &&
    timeToDepleteMs < pace.remainingMs;
  const isTight = !willDeplete && projectedEndPercent >= TIGHT_THRESHOLD_PERCENT;
  return {
    projectedEndPercent,
    projectedRemainingPercent,
    timeToDepleteMs,
    depleteAtUnixMs: willDeplete && timeToDepleteMs != null ? nowMs + timeToDepleteMs : null,
    willDeplete,
    isTight,
  };
}

/**
 * Estima ritmo de gasto em saldo/créditos (ex.: OpenRouter em USD) a partir
 * dos pontos históricos, ignorando recargas (aumentos de saldo).
 */
export function computeCreditsBurnEstimate(samples: TimedSample[]): CreditsBurnEstimate | null {
  if (samples.length < 2) return null;
  const sorted = [...samples].sort((a, b) => a.at - b.at);
  const spanMs = sorted[sorted.length - 1].at - sorted[0].at;
  if (spanMs < 6 * HOUR_MS) return null;

  let totalSpent = 0;
  for (let i = 1; i < sorted.length; i += 1) {
    const spent = sorted[i - 1].value - sorted[i].value;
    if (spent > 0) totalSpent += spent;
  }
  if (totalSpent <= 0) return null;

  const spanDays = spanMs / DAY_MS;
  const ratePerDay = totalSpent / spanDays;
  const currentBalance = sorted[sorted.length - 1].value;
  const daysRemaining = currentBalance > 0 && ratePerDay > 0 ? currentBalance / ratePerDay : null;

  return { ratePerDay, daysRemaining, spanDays };
}

/** Formata a velocidade de queima em pt-BR: `3,0%/dia`, `18%/h`, `<0,1%/h`. */
export function formatPaceRate(ratePerUnit: number, unit: PaceRateUnit): string {
  const suffix = unit === "hour" ? "%/h" : "%/dia";
  if (ratePerUnit > 0 && ratePerUnit < 0.05) return `<0,1${suffix}`;
  const formatted = ratePerUnit.toLocaleString("pt-BR", {
    minimumFractionDigits: ratePerUnit < 10 ? 1 : 0,
    maximumFractionDigits: ratePerUnit < 10 ? 1 : 0,
  });
  return `${formatted}${suffix}`;
}

/** Formata duração de projeção de forma curta e legível: `45min`, `2h 15min`, `1d 12h`, `10d`. */
export function formatPaceDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "agora";
  const totalMinutes = Math.max(1, Math.round(ms / 60_000));
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days >= 10) return `${days}d`;
  if (days > 0) return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  if (hours > 0) return minutes > 0 ? `${hours}h ${minutes}min` : `${hours}h`;
  return `${minutes}min`;
}

/** Formata duração ultra-compacta para células da matriz: `45m`, `2h15`, `1d12h`, `10d`. */
export function formatCompactDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "agora";
  const totalMinutes = Math.max(1, Math.round(ms / 60_000));
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  if (days >= 10) return `${days}d`;
  if (days > 0) return hours > 0 ? `${days}d${hours}h` : `${days}d`;
  if (hours > 0) return minutes > 0 ? `${hours}h${String(minutes).padStart(2, "0")}` : `${hours}h`;
  return `${minutes}min`;
}

/** Formata data/hora local de um timestamp futuro: `hoje, 14:30`, `amanhã, 08:00` ou `25/11, 08:00`. */
export function formatResetDateTime(timestampMs: number, nowMs = Date.now()): string {
  const target = new Date(timestampMs);
  const now = new Date(nowMs);
  const time = target.toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });

  const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const dayDiff = Math.round((startOfDay(target) - startOfDay(now)) / DAY_MS);

  if (dayDiff === 0) return `hoje, ${time}`;
  if (dayDiff === 1) return `amanhã, ${time}`;
  const dd = String(target.getDate()).padStart(2, "0");
  const mm = String(target.getMonth() + 1).padStart(2, "0");
  return `${dd}/${mm}, ${time}`;
}

/** Texto destacado de renovação para cota esgotada (`0% restante`). */
export function formatExhaustedResetSummary(
  quota: Pick<QuotaWindow, "resetsAtUnixSeconds" | "resetDescription">,
  nowMs = Date.now(),
): string {
  const ts =
    typeof quota.resetsAtUnixSeconds === "number" &&
    Number.isFinite(quota.resetsAtUnixSeconds) &&
    quota.resetsAtUnixSeconds > 0
      ? quota.resetsAtUnixSeconds
      : null;
  if (ts != null && ts * 1000 > nowMs) {
    const countdown = formatReset(ts, nowMs).replace(/^Reseta\s+/i, "");
    const dateLabel = formatResetDateTime(ts * 1000, nowMs);
    return `Renova ${countdown} (${dateLabel})`;
  }
  const desc = quota.resetDescription?.trim();
  if (desc) return `Renova: ${desc}`;
  return "Horário de reset não informado";
}

function buildRecentPaceBadge(
  pace: QuotaPaceEstimate,
  cycleRateText: string,
  recentPace: RecentHistoryPace | null | undefined,
  nowMs: number,
): MeterPaceRecentBadge | null {
  if (!recentPace || recentPace.trend === "steady") return null;
  const recentRateText =
    recentPace.ratePerUnit > 0
      ? `~${formatPaceRate(recentPace.ratePerUnit, recentPace.rateUnit)}`
      : formatPaceRate(0, recentPace.rateUnit);
  const projection = projectPaceAtRecentRate(pace, recentPace, nowMs);

  if (recentPace.trend === "faster") {
    const depletesAtRecentPace = Boolean(projection?.willDeplete);
    const escalatedRisk = depletesAtRecentPace && pace.status !== "will-deplete";
    const text = escalatedRisk
      ? `↑ acelerou (${recentRateText} · esgota antes)`
      : `↑ acelerou (${recentRateText})`;
    let detail = `Nos ${recentPace.windowLabel}, o consumo acelerou para ${recentRateText} (média do ciclo: ${cycleRateText}).`;
    if (projection?.willDeplete && projection.timeToDepleteMs != null) {
      const endsIn = formatPaceDuration(projection.timeToDepleteMs);
      const depleteAt =
        projection.depleteAtUnixMs != null
          ? ` (previsto para ${formatResetDateTime(projection.depleteAtUnixMs, nowMs)})`
          : "";
      detail += escalatedRisk
        ? ` Mantendo o passo recente, a cota esgotaria em ~${endsIn}${depleteAt}, antes do reset — mesmo com a média do ciclo parecendo sobrar.`
        : ` Mantendo o passo recente, a cota esgota ainda mais cedo: em ~${endsIn}${depleteAt}.`;
    } else if (projection?.isTight) {
      detail += ` Mantendo o passo recente, a cota chega no limite até o reset (sobra ~${Math.round(projection.projectedRemainingPercent)}%).`;
    } else if (projection) {
      detail += ` Mesmo no passo recente, a cota ainda dura até o reset (sobra ~${Math.round(projection.projectedRemainingPercent)}%).`;
    }
    return {
      trend: "faster",
      tone: depletesAtRecentPace ? "danger" : "warning",
      text,
      tooltip: detail,
      depletesAtRecentPace,
    };
  }

  const depletesAtRecentPace = Boolean(projection?.willDeplete);
  let detail = `Nos ${recentPace.windowLabel}, o consumo desacelerou para ${recentRateText} (média do ciclo: ${cycleRateText}).`;
  if (pace.status === "will-deplete" && projection && !projection.willDeplete) {
    detail += ` Mantendo o passo recente, a cota passa a durar até o reset (sobra ~${Math.round(projection.projectedRemainingPercent)}%).`;
  } else if (projection?.willDeplete && projection.timeToDepleteMs != null) {
    detail += ` Mesmo desacelerando, no passo recente ainda esgotaria em ~${formatPaceDuration(projection.timeToDepleteMs)}.`;
  } else if (projection) {
    detail += ` Mantendo o passo recente, a sobra estimada no reset sobe para ~${Math.round(projection.projectedRemainingPercent)}%.`;
  }
  return {
    trend: "slower",
    tone: "ok",
    text: "↓ desacelerou",
    tooltip: detail,
    depletesAtRecentPace,
  };
}

/** Resumo curto para a linha abaixo da barra em `QuotaMeter`. */
export function formatMeterPaceLine(
  pace: QuotaPaceEstimate,
  recentPace?: RecentHistoryPace | null,
  nowMs = Date.now(),
): {
  tone: "ok" | "warning" | "danger";
  rateText: string;
  verdictText: string;
  tooltip: string;
  recentBadge: MeterPaceRecentBadge | null;
} | null {
  if (pace.ratePerUnit == null) return null;
  const rateText = `~${formatPaceRate(pace.ratePerUnit, pace.rateUnit)}`;
  const recentBadge = buildRecentPaceBadge(pace, rateText, recentPace, nowMs);
  const withRecentTooltip = (baseTooltip: string) =>
    recentBadge ? `${baseTooltip} ${recentBadge.tooltip}` : baseTooltip;

  if (pace.status === "will-deplete" && pace.timeToDepleteMs != null) {
    const endsIn = formatPaceDuration(pace.timeToDepleteMs);
    const beforeMs = pace.marginMs != null ? Math.abs(pace.marginMs) : null;
    const beforeText = beforeMs != null && beforeMs >= 10 * 60_000 ? ` (${formatPaceDuration(beforeMs)} antes do reset)` : "";
    const depleteAtText = pace.depleteAtUnixMs != null ? ` · previsto para ${formatResetDateTime(pace.depleteAtUnixMs, nowMs)}` : "";
    return {
      tone: "danger",
      rateText,
      verdictText: `esgota em ~${endsIn}`,
      tooltip: withRecentTooltip(`No ritmo atual (${rateText}), a cota acaba em ~${endsIn}${beforeText}${depleteAtText}.`),
      recentBadge,
    };
  }

  if (pace.status === "tight" && pace.projectedRemainingPercent != null) {
    const left = Math.round(pace.projectedRemainingPercent);
    return {
      tone: "warning",
      rateText,
      verdictText: left > 0 ? `no limite (sobra ~${left}%)` : "no limite até o reset",
      tooltip: withRecentTooltip(`No ritmo atual (${rateText}), a cota chega perto do limite no fechamento do ciclo (sobra estimada de ~${left}%).`),
      recentBadge,
    };
  }

  if (pace.status === "ok" && pace.projectedRemainingPercent != null) {
    const left = Math.round(pace.projectedRemainingPercent);
    return {
      tone: "ok",
      rateText,
      verdictText: `dura até o reset (sobra ~${left}%)`,
      tooltip: withRecentTooltip(`No ritmo atual (${rateText}), a cota dura até o reset com folga estimada de ~${left}%.`),
      recentBadge,
    };
  }

  return null;
}

/** Resumo compacto para célula da matriz modelo × janela (Antigravity). */
export function formatMatrixPaceHint(
  pace: QuotaPaceEstimate | null,
  quota: Pick<QuotaWindow, "resetsAtUnixSeconds" | "resetDescription">,
  nowMs = Date.now(),
): { tone: "ok" | "warning" | "danger"; text: string; tooltip: string } | null {
  if (!pace) return null;
  if (pace.status === "exhausted") {
    const countdown = formatCountdown(quota.resetsAtUnixSeconds, nowMs);
    const full = formatExhaustedResetSummary(quota, nowMs);
    return {
      tone: "danger",
      text: countdown && countdown !== "renovado" ? `Esgotada · ↻ ${countdown}` : "Esgotada",
      tooltip: `Cota esgotada · ${full}`,
    };
  }
  if (pace.ratePerUnit == null) return null;
  const rateText = `~${formatPaceRate(pace.ratePerUnit, pace.rateUnit)}`;
  if (pace.status === "will-deplete" && pace.timeToDepleteMs != null) {
    const compact = formatCompactDuration(pace.timeToDepleteMs);
    return {
      tone: "danger",
      text: `acaba ~${compact}`,
      tooltip: `Ritmo ${rateText} — no passo atual, acaba em ~${formatPaceDuration(pace.timeToDepleteMs)} (antes do reset).`,
    };
  }
  if (pace.status === "tight" && pace.projectedRemainingPercent != null) {
    const left = Math.round(pace.projectedRemainingPercent);
    return {
      tone: "warning",
      text: `${rateText.replace("/dia", "/d")} · limite`,
      tooltip: `Ritmo ${rateText} — chega no limite até o reset (sobra ~${left}%).`,
    };
  }
  if (pace.status === "ok" && pace.projectedRemainingPercent != null) {
    const left = Math.round(pace.projectedRemainingPercent);
    return {
      tone: "ok",
      text: `${rateText.replace("/dia", "/d")} · dura`,
      tooltip: `Ritmo ${rateText} — dura até o reset (sobra ~${left}%).`,
    };
  }
  return null;
}
