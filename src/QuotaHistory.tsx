import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";

export const QUOTA_HISTORY_DAYS = 30;
export const CREDITS_WINDOW_ID = "__credits__";
const HISTORY_CACHE_TTL_MS = 5 * 60 * 1000;
const MAX_RENDER_POINTS = 90;

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

const historyCache = new Map<string, { at: number; points: QuotaHistoryPoint[] }>();

export function quotaHistoryCacheKey(providerId: string, windowId: string): string {
  return `${providerId}|${windowId}`;
}

export async function fetchQuotaHistory(
  providerId: string,
  windowId: string,
  days = QUOTA_HISTORY_DAYS,
): Promise<QuotaHistoryPoint[]> {
  const key = quotaHistoryCacheKey(providerId, windowId);
  const cached = historyCache.get(key);
  if (cached && Date.now() - cached.at < HISTORY_CACHE_TTL_MS) return cached.points;
  const response = await invoke<QuotaHistoryResponse>("get_quota_history", {
    providerId,
    windowId,
    days,
  });
  const points = Array.isArray(response?.points) ? response.points : [];
  historyCache.set(key, { at: Date.now(), points });
  return points;
}

export function clearQuotaHistoryCache(): void {
  historyCache.clear();
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

export function QuotaHistoryChart({
  providerId,
  windowId,
  windowLabel,
  mode,
  formatValue = defaultPercentFormat,
}: {
  providerId: string;
  windowId: string;
  windowLabel: string;
  mode: QuotaHistoryMode;
  formatValue?: (value: number) => string;
}) {
  const [points, setPoints] = useState<QuotaHistoryPoint[] | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const plotRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let disposed = false;
    setPoints(null);
    setFailure(null);
    setHoverIndex(null);
    fetchQuotaHistory(providerId, windowId)
      .then(result => {
        if (!disposed) setPoints(result);
      })
      .catch(() => {
        if (!disposed) setFailure("Não foi possível ler o histórico local.");
      });
    return () => {
      disposed = true;
    };
  }, [providerId, windowId]);

  const values = useMemo(
    () => (points ? downsampleValues(toTimedValues(points, mode), MAX_RENDER_POINTS) : []),
    [points, mode],
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
    return (
      <p className="quota-history-empty" role="status">
        Sem histórico ainda. Cada atualização de cotas registra um ponto — volte após as próximas leituras.
      </p>
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
  const span = Math.max(1, last - first);
  const x = (at: number) => padLeft + ((at - first) / span) * plotWidth;
  const y = (value: number) => padTop + plotHeight - (Math.max(0, value) / scale) * plotHeight;

  const line = values.map((entry, index) => `${index === 0 ? "M" : "L"}${x(entry.at).toFixed(1)},${y(entry.value).toFixed(1)}`).join(" ");
  const area = `${line} L${x(last).toFixed(1)},${(padTop + plotHeight).toFixed(1)} L${x(first).toFixed(1)},${(padTop + plotHeight).toFixed(1)} Z`;
  const peakIndex = values.reduce((best, entry, index) => (entry.value > values[best].value ? index : best), 0);

  const hovered = hoverIndex != null ? values[hoverIndex] : null;
  const accent = "var(--quota-accent, #83a9c4)";
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
          const ratio = (event.clientX - rect.left) / rect.width;
          const plotRatio = (ratio * width - padLeft) / plotWidth;
          const index = Math.round(plotRatio * (values.length - 1));
          setHoverIndex(Math.max(0, Math.min(values.length - 1, index)));
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
                stroke="rgba(255,255,255,0.08)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
              <text x={2} y={y(tick) + 3} className="quota-history-tick">{tickLabel(tick)}</text>
            </g>
          ))}
          <path d={area} fill={accent} opacity="0.18" />
          <path d={line} fill="none" stroke={accent} strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
          <circle cx={x(values[peakIndex].at)} cy={y(values[peakIndex].value)} r="3.5" fill={accent} stroke="#141619" strokeWidth="1.5">
            <title>{`Pico: ${formatValue(values[peakIndex].value)} em ${formatDayMonth(values[peakIndex].at)}`}</title>
          </circle>
          {hovered && (
            <g>
              <line
                x1={x(hovered.at)}
                x2={x(hovered.at)}
                y1={padTop}
                y2={padTop + plotHeight}
                stroke="rgba(255,255,255,0.35)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
              <circle cx={x(hovered.at)} cy={y(hovered.value)} r="3" fill="#fff" stroke={accent} strokeWidth="2" />
            </g>
          )}
          <text x={padLeft} y={height - 3} className="quota-history-tick">{formatDayMonth(first)}</text>
          <text x={width - padRight} y={height - 3} textAnchor="end" className="quota-history-tick">
            {values.length > 1 ? formatDayMonth(last) : "hoje"}
          </text>
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
  formatValue,
}: {
  providerId: string;
  windowId: string;
  windowLabel: string;
  mode: QuotaHistoryMode;
  formatValue?: (value: number) => string;
}) {
  const [open, setOpen] = useState(false);
  const closeTimer = useRef<number | undefined>(undefined);
  const wrapRef = useRef<HTMLSpanElement | null>(null);

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

  const dialogId = `quota-history-${providerId}-${windowId}`;
  const label = `Ver histórico de uso de ${windowLabel} — últimos ${QUOTA_HISTORY_DAYS} dias`;

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
        <div id={dialogId} className="quota-history-popover" role="dialog" aria-label={`Histórico de uso — ${windowLabel}`}>
          <p className="quota-history-popover-title">
            {windowLabel} <span>ÚLTIMOS {QUOTA_HISTORY_DAYS} DIAS</span>
          </p>
          <QuotaHistoryChart
            providerId={providerId}
            windowId={windowId}
            windowLabel={windowLabel}
            mode={mode}
            formatValue={formatValue}
          />
        </div>
      )}
    </span>
  );
}
