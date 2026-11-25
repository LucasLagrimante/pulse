import { invoke } from "@tauri-apps/api/core";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  computeCreditsBurnEstimate,
  computeQuotaPace,
  computeRecentHistoryPace,
  formatExhaustedResetSummary,
  formatPaceDuration,
  formatPaceRate,
  formatResetDateTime,
  projectPaceAtRecentRate,
  type CreditsBurnEstimate,
  type QuotaPaceEstimate,
  type RecentHistoryPace,
} from "./quotaPace";

export const QUOTA_HISTORY_DAYS = 30;
export const RECENT_PACE_HISTORY_DAYS = 4;
export const CREDITS_WINDOW_ID = "__credits__";
const POPOVER_WIDTH = 344;
const POPOVER_HEIGHT = 340;
const POPOVER_MARGIN = 8;
const HISTORY_CACHE_TTL_MS = 5 * 60 * 1000;
const MAX_RENDER_POINTS = 90;
// Abaixo do teto de 6000 pontos do backend: garante que o lote recente não foi podado.
const BULK_COMPLETE_POINT_LIMIT = 5900;

export type QuotaHistoryPoint = {
  providerId: string;
  windowId: string;
  label: string;
  usedPercent?: number | null;
  remainingPercent?: number | null;
  creditsRemaining?: number | null;
  sampledAtUnixMs: number;
};

export type QuotaHistoryResponse = {
  points: QuotaHistoryPoint[];
  retainedDays: number;
};

export type QuotaHistoryMode = "percent" | "credits";

function finiteNumber(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Valor da série: % usado no modo percent, saldo/créditos no modo credits. */
export function historyPointValue(point: QuotaHistoryPoint, mode: QuotaHistoryMode): number | null {
  if (mode === "credits") return finiteNumber(point.creditsRemaining);
  const used = finiteNumber(point.usedPercent);
  if (used != null) return used;
  const remaining = finiteNumber(point.remainingPercent);
  return remaining != null ? 100 - remaining : null;
}

export type TimedValue = { at: number; value: number };

export function toTimedValues(points: QuotaHistoryPoint[], mode: QuotaHistoryMode): TimedValue[] {
  return points
    .filter(point => typeof point.sampledAtUnixMs === "number" && point.sampledAtUnixMs > 0)
    .map(point => ({ at: point.sampledAtUnixMs, value: historyPointValue(point, mode) }))
    .filter((entry): entry is TimedValue => entry.value != null)
    .sort((a, b) => a.at - b.at);
}

export type DayBucket = {
  dayKey: string;
  label: string;
  max: number;
  avg: number;
  count: number;
};

function dayKeyOf(at: number): string {
  const date = new Date(at);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

export function groupValuesByDay(values: TimedValue[]): DayBucket[] {
  const buckets = new Map<string, { sum: number; max: number; count: number }>();
  for (const entry of values) {
    const key = dayKeyOf(entry.at);
    const current = buckets.get(key);
    if (!current) {
      buckets.set(key, { sum: entry.value, max: entry.value, count: 1 });
    } else {
      current.sum += entry.value;
      current.max = Math.max(current.max, entry.value);
      current.count += 1;
    }
  }
  return [...buckets.entries()]
    .sort((a, b) => (a[0] < b[0] ? -1 : 1))
    .map(([dayKey, bucket]) => ({
      dayKey,
      label: `${dayKey.slice(8, 10)}/${dayKey.slice(5, 7)}`,
      max: bucket.max,
      avg: bucket.sum / bucket.count,
      count: bucket.count,
    }));
}

/** Dia de maior valor (empate: o mais recente). */
export function peakBucket(buckets: DayBucket[]): DayBucket | null {
  let peak: DayBucket | null = null;
  for (const bucket of buckets) {
    if (!peak || bucket.max > peak.max || (bucket.max === peak.max && bucket.dayKey > peak.dayKey)) {
      peak = bucket;
    }
  }
  return peak;
}

/** Reduz a série preservando primeira e última amostra. */
export function downsampleValues(values: TimedValue[], maxPoints: number): TimedValue[] {
  if (values.length <= maxPoints || maxPoints < 2) return values;
  const stride = (values.length - 1) / (maxPoints - 1);
  const sampled: TimedValue[] = [];
  for (let i = 0; i < maxPoints; i += 1) {
    sampled.push(values[Math.round(i * stride)]);
  }
  return sampled;
}

export type QuotaWindowInfo = {
  id?: string;
  label?: string;
  usedPercent?: number | null;
  remainingPercent?: number | null;
  windowMinutes?: number | null;
  resetsAtUnixSeconds?: number | null;
  resetDescription?: string | null;
};

export type QuotaCycleInfo = {
  cycleStartMs: number;
  cycleLabel: string;
  popoverBadge: string;
};

export function computeCycleInfo(
  quota?: QuotaWindowInfo | null,
  mode: QuotaHistoryMode = "percent",
  nowMs = Date.now(),
): QuotaCycleInfo {
  if (mode === "credits" || !quota) {
    return {
      cycleStartMs: nowMs - QUOTA_HISTORY_DAYS * 86_400_000,
      cycleLabel: `últimos ${QUOTA_HISTORY_DAYS} dias`,
      popoverBadge: `ÚLTIMOS ${QUOTA_HISTORY_DAYS} DIAS`,
    };
  }

  const rawMinutes = typeof quota.windowMinutes === "number" && quota.windowMinutes > 0
    ? quota.windowMinutes
    : null;
  const resetsAtSeconds = typeof quota.resetsAtUnixSeconds === "number" && Number.isFinite(quota.resetsAtUnixSeconds)
    ? quota.resetsAtUnixSeconds
    : null;

  const labelLower = (quota.label || "").toLowerCase();
  const idLower = (quota.id || "").toLowerCase();

  const isWeekly = rawMinutes === 10080 ||
    labelLower.includes("sem") ||
    idLower.includes("week") ||
    labelLower.includes("7d") ||
    labelLower.includes("7 dia");

  const isMonthly = rawMinutes === 43200 ||
    labelLower.includes("mens") ||
    idLower.includes("month") ||
    labelLower.includes("30d");

  const isFiveHours = (rawMinutes != null && rawMinutes <= 300) ||
    labelLower.includes("5h") ||
    labelLower.includes("5 hora") ||
    labelLower.includes("sess") ||
    idLower.includes("sess");

  const minutes = rawMinutes ?? (isFiveHours ? 300 : isWeekly ? 10080 : isMonthly ? 43200 : null);

  // Se a janela já expirou pelo timestamp de reset, o novo ciclo começou exatamente no reset
  if (resetsAtSeconds != null && resetsAtSeconds * 1000 <= nowMs) {
    return {
      cycleStartMs: resetsAtSeconds * 1000,
      cycleLabel: isFiveHours ? "janela atual (5h)" : isWeekly ? "ciclo semanal renovado" : isMonthly ? "ciclo mensal renovado" : "ciclo renovado",
      popoverBadge: isFiveHours ? "JANELA ATUAL (5H)" : isWeekly ? "CICLO SEMANAL (RENOVADO)" : isMonthly ? "CICLO MENSAL (RENOVADO)" : "CICLO RENOVADO",
    };
  }

  if (resetsAtSeconds != null && minutes != null) {
    const periodDurationMs = minutes * 60_000;
    const periodStartMs = resetsAtSeconds * 1000 - periodDurationMs;
    const label = isFiveHours ? "janela atual (5h)" : isWeekly ? "ciclo semanal" : isMonthly ? "ciclo mensal" : "ciclo atual";
    const badge = isFiveHours ? "JANELA ATUAL (5H)" : isWeekly ? "CICLO SEMANAL (7 DIAS)" : isMonthly ? "CICLO MENSAL" : "CICLO ATUAL";
    return {
      cycleStartMs: Math.max(0, periodStartMs),
      cycleLabel: label,
      popoverBadge: badge,
    };
  }

  if (isFiveHours) {
    return {
      cycleStartMs: nowMs - (minutes ?? 300) * 60_000,
      cycleLabel: "janela atual (5h)",
      popoverBadge: "JANELA ATUAL (5H)",
    };
  }

  if (isWeekly) {
    return {
      cycleStartMs: nowMs - 7 * 86_400_000,
      cycleLabel: "ciclo semanal",
      popoverBadge: "CICLO SEMANAL (7 DIAS)",
    };
  }

  if (isMonthly) {
    return {
      cycleStartMs: nowMs - 30 * 86_400_000,
      cycleLabel: "ciclo mensal",
      popoverBadge: "CICLO MENSAL",
    };
  }

  return {
    cycleStartMs: nowMs - QUOTA_HISTORY_DAYS * 86_400_000,
    cycleLabel: `últimos ${QUOTA_HISTORY_DAYS} dias`,
    popoverBadge: `ÚLTIMOS ${QUOTA_HISTORY_DAYS} DIAS`,
  };
}

type RecentHistoryCacheEntry = {
  at: number;
  coveredSinceMs: number;
  isComplete: boolean;
  bySeries: Map<string, QuotaHistoryPoint[]>;
};

const historyCache = new Map<string, { at: number; points: QuotaHistoryPoint[] }>();
const historyInFlight = new Map<string, Promise<QuotaHistoryPoint[]>>();
let recentHistoryCache: RecentHistoryCacheEntry | null = null;
let recentHistoryInFlight: Promise<Map<string, QuotaHistoryPoint[]>> | null = null;

export function quotaSeriesKey(providerId: string, windowId: string): string {
  return `${providerId}|${windowId}`;
}

export function quotaHistoryCacheKey(providerId: string, windowId: string, sinceUnixMs?: number | null): string {
  return `${providerId}|${windowId}|${sinceUnixMs ?? 0}`;
}

function groupPointsBySeries(points: QuotaHistoryPoint[]): Map<string, QuotaHistoryPoint[]> {
  const bySeries = new Map<string, QuotaHistoryPoint[]>();
  for (const point of points) {
    if (!point || typeof point.providerId !== "string" || typeof point.windowId !== "string") continue;
    const key = quotaSeriesKey(point.providerId, point.windowId);
    const list = bySeries.get(key);
    if (list) list.push(point);
    else bySeries.set(key, [point]);
  }
  return bySeries;
}

/**
 * Carrega o histórico recente (últimos 4 dias) de todos os provedores em uma
 * única chamada IPC, com cache compartilhado e deduplicação em voo.
 */
export async function fetchRecentQuotaHistory(
  forceRefresh = false,
): Promise<Map<string, QuotaHistoryPoint[]>> {
  const now = Date.now();
  if (!forceRefresh && recentHistoryCache && now - recentHistoryCache.at < HISTORY_CACHE_TTL_MS) {
    return recentHistoryCache.bySeries;
  }
  if (!forceRefresh && recentHistoryInFlight) {
    return recentHistoryInFlight;
  }

  const coveredSinceMs = now - RECENT_PACE_HISTORY_DAYS * 86_400_000;
  const request = invoke<QuotaHistoryResponse>("get_quota_history", {
    providerId: null,
    windowId: null,
    days: RECENT_PACE_HISTORY_DAYS,
    sinceUnixMs: null,
  })
    .then(response => {
      const points = Array.isArray(response?.points) ? response.points : [];
      const bySeries = groupPointsBySeries(points);
      recentHistoryCache = {
        at: Date.now(),
        coveredSinceMs,
        isComplete: points.length < BULK_COMPLETE_POINT_LIMIT,
        bySeries,
      };
      return bySeries;
    })
    .finally(() => {
      if (recentHistoryInFlight === request) recentHistoryInFlight = null;
    });

  recentHistoryInFlight = request;
  return request;
}

export async function fetchQuotaHistory(
  providerId: string,
  windowId: string,
  days = QUOTA_HISTORY_DAYS,
  sinceUnixMs?: number | null,
): Promise<QuotaHistoryPoint[]> {
  const key = quotaHistoryCacheKey(providerId, windowId, sinceUnixMs);
  const now = Date.now();
  const cached = historyCache.get(key);
  if (cached && now - cached.at < HISTORY_CACHE_TTL_MS) return cached.points;

  // Reaproveita o lote recente quando o ciclo pedido cabe inteiro na janela já carregada.
  if (
    recentHistoryCache &&
    recentHistoryCache.isComplete &&
    now - recentHistoryCache.at < HISTORY_CACHE_TTL_MS &&
    sinceUnixMs != null &&
    sinceUnixMs >= recentHistoryCache.coveredSinceMs
  ) {
    const series = recentHistoryCache.bySeries.get(quotaSeriesKey(providerId, windowId)) ?? [];
    const filtered = series.filter(point => point.sampledAtUnixMs >= sinceUnixMs);
    historyCache.set(key, { at: now, points: filtered });
    return filtered;
  }

  const existing = historyInFlight.get(key);
  if (existing) return existing;

  const request = invoke<QuotaHistoryResponse>("get_quota_history", {
    providerId,
    windowId,
    days,
    sinceUnixMs: sinceUnixMs ?? null,
  })
    .then(response => {
      const points = Array.isArray(response?.points) ? response.points : [];
      historyCache.set(key, { at: Date.now(), points });
      return points;
    })
    .finally(() => {
      if (historyInFlight.get(key) === request) historyInFlight.delete(key);
    });

  historyInFlight.set(key, request);
  return request;
}

export function clearQuotaHistoryCache(): void {
  historyCache.clear();
  historyInFlight.clear();
  recentHistoryCache = null;
  recentHistoryInFlight = null;
}

function formatDayMonth(at: number): string {
  const date = new Date(at);
  return `${String(date.getDate()).padStart(2, "0")}/${String(date.getMonth() + 1).padStart(2, "0")}`;
}

function formatHour(at: number): string {
  return new Date(at).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
}

export function defaultPercentFormat(value: number): string {
  return `${Math.round(value)}%`;
}

function ChartIcon() {
  return (
    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M3 3v16a2 2 0 0 0 2 2h16" />
      <path d="M7 14l4-4 3 3 5-6" />
    </svg>
  );
}

function QuotaHistoryPacePanel({
  pace,
  quota,
  recentPace,
  creditsBurn,
  mode,
  formatValue,
}: {
  pace: QuotaPaceEstimate | null;
  quota?: QuotaWindowInfo | null;
  recentPace: RecentHistoryPace | null;
  creditsBurn: CreditsBurnEstimate | null;
  mode: QuotaHistoryMode;
  formatValue: (value: number) => string;
}) {
  if (mode === "credits") {
    if (!creditsBurn) return null;
    const rateLabel = `${formatValue(creditsBurn.ratePerDay)}/dia`;
    const runway =
      creditsBurn.daysRemaining != null
        ? `dura ~${formatPaceDuration(creditsBurn.daysRemaining * 86_400_000)}`
        : "saldo zerado";
    const tone =
      creditsBurn.daysRemaining == null
        ? "danger"
        : creditsBurn.daysRemaining < 7
          ? "warning"
          : "ok";
    return (
      <div className={`quota-history-pace is-${tone}`} role="status">
        <div className="quota-history-pace-head">
          <i className="quota-pace-dot" aria-hidden="true" />
          <strong>Ritmo de saldo</strong>
          <span>~{rateLabel}</span>
        </div>
        <p className="quota-history-pace-detail">
          No passo dos últimos {Math.max(1, Math.round(creditsBurn.spanDays))}d, o saldo atual {runway}.
        </p>
      </div>
    );
  }

  if (!pace) return null;

  if (pace.status === "exhausted" && quota) {
    return (
      <div className="quota-history-pace is-danger" role="status">
        <div className="quota-history-pace-head">
          <i className="quota-pace-dot" aria-hidden="true" />
          <strong>Cota esgotada</strong>
          <span>100% usado</span>
        </div>
        <p className="quota-history-pace-detail">{formatExhaustedResetSummary(quota)}</p>
      </div>
    );
  }

  const renderRecentPaceNote = () => {
    if (!recentPace || recentPace.trend === "steady") return null;
    const recentRate =
      recentPace.ratePerUnit > 0
        ? `~${formatPaceRate(recentPace.ratePerUnit, recentPace.rateUnit)}`
        : formatPaceRate(0, recentPace.rateUnit);
    const recentProjection = projectPaceAtRecentRate(pace, recentPace);
    const escalatedRisk =
      recentPace.trend === "faster" &&
      pace.status !== "will-deplete" &&
      Boolean(recentProjection?.willDeplete) &&
      recentProjection?.timeToDepleteMs != null;
    const suffix = escalatedRisk && recentProjection?.timeToDepleteMs != null
      ? `acelerou · esgotaria em ~${formatPaceDuration(recentProjection.timeToDepleteMs)}`
      : recentPace.trend === "faster"
        ? "acelerou"
        : "desacelerou";
    return (
      <p className={`quota-history-pace-recent${escalatedRisk ? " is-danger" : ""}`}>
        Nos {recentPace.windowLabel}: {recentRate} ({suffix})
      </p>
    );
  };

  if (pace.status === "will-deplete" && pace.ratePerUnit != null && pace.timeToDepleteMs != null) {
    const rateText = formatPaceRate(pace.ratePerUnit, pace.rateUnit);
    const endsIn = formatPaceDuration(pace.timeToDepleteMs);
    const beforeMs = pace.marginMs != null ? Math.abs(pace.marginMs) : null;
    const beforeText =
      beforeMs != null && beforeMs >= 10 * 60_000
        ? ` · ${formatPaceDuration(beforeMs)} antes do reset`
        : "";
    const depleteAtText =
      pace.depleteAtUnixMs != null ? ` (${formatResetDateTime(pace.depleteAtUnixMs)})` : "";
    return (
      <div className="quota-history-pace is-danger" role="status">
        <div className="quota-history-pace-head">
          <i className="quota-pace-dot" aria-hidden="true" />
          <strong>Vai esgotar antes do reset</strong>
          <span>~{rateText}</span>
        </div>
        <p className="quota-history-pace-detail">
          Esgota em ~{endsIn}{depleteAtText}{beforeText}
        </p>
        {renderRecentPaceNote()}
      </div>
    );
  }

  if ((pace.status === "tight" || pace.status === "ok") && pace.ratePerUnit != null && pace.projectedRemainingPercent != null) {
    const rateText = formatPaceRate(pace.ratePerUnit, pace.rateUnit);
    const projectedUsed = Math.min(100, Math.round(pace.projectedEndPercent ?? 0));
    const projectedLeft = Math.round(pace.projectedRemainingPercent);
    const isTight = pace.status === "tight";
    return (
      <div className={`quota-history-pace is-${isTight ? "warning" : "ok"}`} role="status">
        <div className="quota-history-pace-head">
          <i className="quota-pace-dot" aria-hidden="true" />
          <strong>{isTight ? "Chega no limite até o reset" : "Dura até o reset"}</strong>
          <span>~{rateText}</span>
        </div>
        <p className="quota-history-pace-detail">
          Projeção de ~{projectedUsed}% no fechamento (sobra ~{projectedLeft}%)
        </p>
        {renderRecentPaceNote()}
      </div>
    );
  }

  if (pace.status === "idle") {
    return (
      <div className="quota-history-pace is-neutral" role="status">
        <p className="quota-history-pace-detail">Sem consumo registrado neste ciclo.</p>
      </div>
    );
  }

  if (pace.status === "insufficient") {
    return (
      <div className="quota-history-pace is-neutral" role="status">
        <p className="quota-history-pace-detail">Início do ciclo — aguarde mais uso para projetar o ritmo.</p>
      </div>
    );
  }

  if (pace.status === "no-reset") {
    return (
      <div className="quota-history-pace is-neutral" role="status">
        <p className="quota-history-pace-detail">Reset não informado pelo provedor — projeção indisponível.</p>
      </div>
    );
  }

  return null;
}

export function QuotaHistoryChart({
  providerId,
  windowId,
  windowLabel,
  mode,
  quota,
  formatValue = defaultPercentFormat,
}: {
  providerId: string;
  windowId: string;
  windowLabel: string;
  mode: QuotaHistoryMode;
  quota?: QuotaWindowInfo | null;
  formatValue?: (value: number) => string;
}) {
  const [points, setPoints] = useState<QuotaHistoryPoint[] | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const plotRef = useRef<HTMLDivElement | null>(null);
  const cycleInfo = useMemo(() => computeCycleInfo(quota, mode), [quota, mode]);

  useEffect(() => {
    let disposed = false;
    setPoints(null);
    setFailure(null);
    setHoverIndex(null);
    fetchQuotaHistory(providerId, windowId, QUOTA_HISTORY_DAYS, cycleInfo.cycleStartMs)
      .then(result => {
        if (!disposed) setPoints(result);
      })
      .catch(() => {
        if (!disposed) setFailure("Não foi possível ler o histórico local.");
      });
    return () => {
      disposed = true;
    };
  }, [providerId, windowId, cycleInfo.cycleStartMs]);

  const rawTimedValues = useMemo(
    () =>
      points
        ? toTimedValues(
            points.filter(point => point.sampledAtUnixMs >= cycleInfo.cycleStartMs),
            mode,
          )
        : [],
    [points, mode, cycleInfo.cycleStartMs],
  );

  const values = useMemo(
    () => downsampleValues(rawTimedValues, MAX_RENDER_POINTS),
    [rawTimedValues],
  );

  const pace = useMemo(
    () =>
      mode === "percent" && quota
        ? computeQuotaPace({
            ...quota,
            id: quota.id ?? windowId,
            label: quota.label ?? windowLabel,
          })
        : null,
    [mode, quota, windowId, windowLabel],
  );

  const recentPace = useMemo(
    () =>
      mode === "percent" && pace?.ratePerUnit != null
        ? computeRecentHistoryPace(rawTimedValues, pace.ratePerUnit, pace.rateUnit, Date.now(), pace.cycleStartMs)
        : null,
    [mode, pace, rawTimedValues],
  );

  const creditsBurn = useMemo(
    () => (mode === "credits" ? computeCreditsBurnEstimate(rawTimedValues) : null),
    [mode, rawTimedValues],
  );

  const buckets = useMemo(() => groupValuesByDay(values), [values]);
  const peak = useMemo(() => peakBucket(buckets), [buckets]);

  const stats = useMemo(() => {
    if (values.length === 0) return null;
    let max = values[0].value;
    let sum = 0;
    for (const entry of values) {
      if (entry.value > max) max = entry.value;
      sum += entry.value;
    }
    return { max, avg: sum / values.length };
  }, [values]);

  if (failure) {
    return <p className="quota-history-empty" role="status">{failure}</p>;
  }
  if (points == null) {
    return (
      <div className="quota-history-loading" role="status" aria-label="Carregando histórico de uso">
        <i aria-hidden="true" />
        <span>LENDO HISTÓRICO…</span>
      </div>
    );
  }
  if (values.length === 0 || stats == null) {
    const showPaceWithoutHistory =
      pace != null &&
      (pace.status === "exhausted" ||
        pace.status === "will-deplete" ||
        pace.status === "tight" ||
        pace.status === "ok");
    return (
      <div className="quota-history-chart">
        {showPaceWithoutHistory && (
          <QuotaHistoryPacePanel
            pace={pace}
            quota={quota}
            recentPace={null}
            creditsBurn={null}
            mode={mode}
            formatValue={formatValue}
          />
        )}
        <p className="quota-history-empty" role="status">
          Sem histórico ainda no ciclo atual. Cada atualização de cotas registra um ponto — volte após as próximas leituras.
        </p>
      </div>
    );
  }

  const width = 320;
  const height = 118;
  const padLeft = 30;
  const padRight = 8;
  const padTop = 8;
  const padBottom = 16;
  const plotWidth = width - padLeft - padRight;
  const plotHeight = height - padTop - padBottom;
  const scale = mode === "percent" ? 100 : Math.max(...values.map(entry => entry.value), 1);
  const first = values[0].at;
  const last = values[values.length - 1].at;
  const lastValue = values[values.length - 1].value;

  const hasProjection =
    mode === "percent" &&
    pace != null &&
    (pace.status === "ok" || pace.status === "tight" || pace.status === "will-deplete") &&
    pace.resetsAtMs != null &&
    pace.resetsAtMs > last;

  const xEnd = hasProjection && pace?.resetsAtMs ? pace.resetsAtMs : last;
  const span = Math.max(1, xEnd - first);
  const x = (at: number) => padLeft + ((at - first) / span) * plotWidth;
  const y = (value: number) => padTop + plotHeight - (Math.max(0, Math.min(scale, value)) / scale) * plotHeight;

  const line = values.map((entry, index) => `${index === 0 ? "M" : "L"}${x(entry.at).toFixed(1)},${y(entry.value).toFixed(1)}`).join(" ");
  const area = `${line} L${x(last).toFixed(1)},${(padTop + plotHeight).toFixed(1)} L${x(first).toFixed(1)},${(padTop + plotHeight).toFixed(1)} Z`;
  const peakIndex = values.reduce((best, entry, index) => (entry.value > values[best].value ? index : best), 0);

  const projection = (() => {
    if (!hasProjection || !pace || pace.resetsAtMs == null) return null;
    const isDeplete = pace.status === "will-deplete" && pace.depleteAtUnixMs != null;
    const targetAt = isDeplete
      ? Math.min(pace.resetsAtMs, Math.max(last, pace.depleteAtUnixMs!))
      : pace.resetsAtMs;
    const targetValue = isDeplete
      ? 100
      : Math.min(100, Math.max(lastValue, pace.projectedEndPercent ?? lastValue));
    const color =
      pace.status === "will-deplete"
        ? "var(--neu-danger)"
        : pace.status === "tight"
          ? "var(--neu-warning)"
          : "var(--neu-success)";
    const title = isDeplete
      ? `Projeção: esgota em ${formatResetDateTime(targetAt)}`
      : `Projeção no reset: ~${Math.round(targetValue)}% usado`;
    return {
      x1: x(last),
      y1: y(lastValue),
      x2: x(targetAt),
      y2: y(targetValue),
      color,
      title,
      isDeplete,
    };
  })();

  const hovered = hoverIndex != null ? values[hoverIndex] : null;
  const accent = "var(--quota-accent, var(--neu-accent))";
  const yTicks = mode === "percent" ? [100, 50, 0] : [scale, scale / 2, 0];
  const tickLabel = (tick: number) =>
    mode === "percent" ? `${Math.round(tick)}%` : formatValue(tick);

  const summary = `Histórico de ${windowLabel}: máximo ${formatValue(stats.max)}, média ${formatValue(stats.avg)}${peak ? `, dia de maior valor ${peak.label}` : ""}.`;

  return (
    <div className="quota-history-chart">
      <div
        ref={plotRef}
        className="quota-history-plot"
        onMouseMove={event => {
          const rect = plotRef.current?.getBoundingClientRect();
          if (!rect || rect.width <= 0 || values.length === 0) return;
          const cursorSvgX = ((event.clientX - rect.left) / rect.width) * width;
          let bestIndex = 0;
          let bestDist = Math.abs(x(values[0].at) - cursorSvgX);
          for (let i = 1; i < values.length; i += 1) {
            const dist = Math.abs(x(values[i].at) - cursorSvgX);
            if (dist < bestDist) {
              bestDist = dist;
              bestIndex = i;
            }
          }
          setHoverIndex(bestIndex);
        }}
        onMouseLeave={() => setHoverIndex(null)}
      >
        <svg
          viewBox={`0 0 ${width} ${height}`}
          preserveAspectRatio="none"
          role="img"
          aria-label={summary}
          className="quota-history-svg"
          shapeRendering="geometricPrecision"
        >
          {yTicks.map(tick => (
            <g key={tick}>
              <line
                x1={padLeft}
                x2={width - padRight}
                y1={y(tick)}
                y2={y(tick)}
                stroke="var(--neu-divider)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
              <text x={2} y={y(tick) + 3} className="quota-history-tick">{tickLabel(tick)}</text>
            </g>
          ))}
          <path d={area} fill={accent} opacity="0.18" />
          <path d={line} fill="none" stroke={accent} strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
          {projection && (
            <g className="quota-history-projection">
              <line
                className="quota-history-projection-line"
                x1={projection.x1}
                y1={projection.y1}
                x2={projection.x2}
                y2={projection.y2}
                style={{ stroke: projection.color }}
                strokeWidth="1.75"
                strokeDasharray="4 3"
                strokeLinecap="round"
                vectorEffect="non-scaling-stroke"
              />
              <circle
                cx={projection.x2}
                cy={projection.y2}
                r="3.2"
                fill={projection.isDeplete ? projection.color : "var(--neu-surface)"}
                stroke={projection.color}
                strokeWidth="1.8"
              >
                <title>{projection.title}</title>
              </circle>
            </g>
          )}
          <circle cx={x(values[peakIndex].at)} cy={y(values[peakIndex].value)} r="3.5" fill={accent} stroke="var(--neu-surface)" strokeWidth="1.5">
            <title>{`Pico: ${formatValue(values[peakIndex].value)} em ${formatDayMonth(values[peakIndex].at)} · ${formatHour(values[peakIndex].at)}`}</title>
          </circle>
          {hovered && (
            <g>
              <line
                x1={x(hovered.at)}
                x2={x(hovered.at)}
                y1={padTop}
                y2={padTop + plotHeight}
                stroke="var(--neu-divider)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
              <circle cx={x(hovered.at)} cy={y(hovered.value)} r="3" fill="var(--neu-surface)" stroke={accent} strokeWidth="2" />
            </g>
          )}
          {(() => {
            const isShortSpan = span <= 24 * 3600 * 1000 || formatDayMonth(first) === formatDayMonth(xEnd);
            const startLabel = isShortSpan ? formatHour(first) : formatDayMonth(first);
            const endLabel = hasProjection
              ? `reset ${isShortSpan ? formatHour(xEnd) : formatDayMonth(xEnd)}`
              : values.length > 1
                ? (isShortSpan ? formatHour(last) : formatDayMonth(last))
                : (isShortSpan ? formatHour(first) : "hoje");
            return (
              <>
                <text x={padLeft} y={height - 3} className="quota-history-tick">{startLabel}</text>
                <text x={width - padRight} y={height - 3} textAnchor="end" className="quota-history-tick">
                  {endLabel}
                </text>
              </>
            );
          })()}
        </svg>
        {hovered && (
          <div
            className="quota-history-tooltip"
            style={{ left: `${Math.min(86, Math.max(0, ((x(hovered.at) / width) * 100)))}%` }}
            role="status"
          >
            <strong>{formatValue(hovered.value)}</strong>
            <span>{formatDayMonth(hovered.at)} · {formatHour(hovered.at)}</span>
          </div>
        )}
      </div>
      <QuotaHistoryPacePanel
        pace={pace}
        quota={quota}
        recentPace={recentPace}
        creditsBurn={creditsBurn}
        mode={mode}
        formatValue={formatValue}
      />
      <div className="quota-history-stats">
        <span>MÁX <b>{formatValue(stats.max)}</b></span>
        <span>MÉDIA <b>{formatValue(stats.avg)}</b></span>
        {peak && <span>DIA DO MÁXIMO <b>{peak.label}</b></span>}
      </div>
    </div>
  );
}

export function QuotaHistoryButton({
  providerId,
  windowId,
  windowLabel,
  mode,
  quota,
  formatValue,
}: {
  providerId: string;
  windowId: string;
  windowLabel: string;
  mode: QuotaHistoryMode;
  quota?: QuotaWindowInfo | null;
  formatValue?: (value: number) => string;
}) {
  const [open, setOpen] = useState(false);
  const [placement, setPlacement] = useState("is-down-left");
  const closeTimer = useRef<number | undefined>(undefined);
  const wrapRef = useRef<HTMLSpanElement | null>(null);
  const cycleInfo = useMemo(() => computeCycleInfo(quota, mode), [quota, mode]);

  const cancelClose = () => {
    if (closeTimer.current !== undefined) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = undefined;
    }
  };
  const scheduleClose = () => {
    cancelClose();
    closeTimer.current = window.setTimeout(() => setOpen(false), 120);
  };

  useEffect(
    () => () => {
      if (closeTimer.current !== undefined) window.clearTimeout(closeTimer.current);
    },
    [],
  );

  // Vira o popover para o lado com espaço (nunca fixo à direita): mede a
  // âncora contra o viewport e escolhe abrir abaixo/acima × esquerda/direita.
  useLayoutEffect(() => {
    if (!open) return;
    const update = () => {
      const anchor = wrapRef.current?.getBoundingClientRect();
      if (!anchor) return;
      const alignRight =
        window.innerWidth - anchor.left < POPOVER_WIDTH + POPOVER_MARGIN &&
        anchor.right >= POPOVER_WIDTH + POPOVER_MARGIN;
      const openUp =
        window.innerHeight - anchor.bottom < POPOVER_HEIGHT + POPOVER_MARGIN &&
        anchor.top >= POPOVER_HEIGHT + POPOVER_MARGIN;
      setPlacement(`is-${openUp ? "up" : "down"}-${alignRight ? "right" : "left"}`);
    };
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, [open]);

  const dialogId = `quota-history-${providerId}-${windowId}`;
  const label = `Ver histórico de uso de ${windowLabel} — ${cycleInfo.cycleLabel}`;

  return (
    <span
      ref={wrapRef}
      className="quota-history-wrap"
      onMouseEnter={() => {
        cancelClose();
        setOpen(true);
      }}
      onMouseLeave={scheduleClose}
    >
      <button
        type="button"
        className="quota-history-btn"
        aria-label={label}
        title={label}
        aria-expanded={open}
        aria-controls={dialogId}
        onClick={() => setOpen(value => !value)}
        onFocus={() => {
          cancelClose();
          setOpen(true);
        }}
        onBlur={event => {
          if (!wrapRef.current?.contains(event.relatedTarget as Node | null)) setOpen(false);
        }}
      >
        <ChartIcon />
      </button>
      {open && (
        <div
          id={dialogId}
          className={`quota-history-popover ${placement}`}
          role="dialog"
          aria-label={`Histórico de uso — ${windowLabel}`}
        >
          <p className="quota-history-popover-title">
            {windowLabel} <span>{cycleInfo.popoverBadge}</span>
          </p>
          <QuotaHistoryChart
            providerId={providerId}
            windowId={windowId}
            windowLabel={windowLabel}
            mode={mode}
            quota={quota}
            formatValue={formatValue}
          />
        </div>
      )}
    </span>
  );
}
