import { memo, useCallback, useEffect, useId, useMemo, useRef, useState, type MouseEvent } from "react";
import { metric } from "./App";

const FALLBACK_WIDTH = 320;
const PLOT_PAD_X = 5;
const PLOT_PAD_TOP = 6;
const PLOT_PAD_BOTTOM = 6;
const MAX_VISIBLE_POINTS = 90;

type ChartPoint = {
  index: number;
  value: number;
  x: number;
  y: number;
};

export type SparklineHoverPoint = {
  index: number;
  value: number;
  xPercent: number;
  totalCount: number;
};

/**
 * Arredonda o teto de auto-escala para um degrau limpo (1, 2, 5 × 10^n),
 * evitando saltos nervosos no eixo Y em métricas abertas (MB/s, RPM, W, GHz).
 */
export function computeNiceCeiling(peak: number, floorMax = 10): number {
  const safePeak = Number.isFinite(peak) && peak > 0 ? peak : 0;
  const target = Math.max(floorMax, safePeak * 1.15);
  const exponent = Math.floor(Math.log10(target));
  const magnitude = 10 ** exponent;
  const normalized = target / magnitude;
  if (normalized <= 1) return magnitude;
  if (normalized <= 2) return 2 * magnitude;
  if (normalized <= 2.5) return 2.5 * magnitude;
  if (normalized <= 5) return 5 * magnitude;
  return 10 * magnitude;
}

function clampValue(v: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, v));
}

/**
 * Gera segmentos Bézier cúbicos monotônicos (Fritsch-Carlson) que passam
 * exatamente por cada amostra sem ultrapassar os extremos locais ou o teto/base.
 */
function buildMonotoneRunPaths(
  run: ChartPoint[],
  baseY: number,
  topY: number,
): { line: string; area: string } {
  if (run.length < 2) return { line: "", area: "" };

  const n = run.length;
  const dx: number[] = new Array(n - 1);
  const slopes: number[] = new Array(n - 1);

  for (let i = 0; i < n - 1; i++) {
    const deltaX = Math.max(0.0001, run[i + 1].x - run[i].x);
    dx[i] = deltaX;
    slopes[i] = (run[i + 1].y - run[i].y) / deltaX;
  }

  const tangents: number[] = new Array(n);
  tangents[0] = slopes[0];
  tangents[n - 1] = slopes[n - 2];

  for (let i = 1; i < n - 1; i++) {
    const prevSlope = slopes[i - 1];
    const nextSlope = slopes[i];
    if (prevSlope * nextSlope <= 0) {
      tangents[i] = 0;
    } else {
      const m = (prevSlope + nextSlope) / 2;
      tangents[i] = m;
    }
  }

  // Limite de Fritsch-Carlson para impedir qualquer overshoot entre amostras.
  for (let i = 0; i < n - 1; i++) {
    const s = slopes[i];
    if (Math.abs(s) < 1e-6) {
      tangents[i] = 0;
      tangents[i + 1] = 0;
    } else {
      const a = tangents[i] / s;
      const b = tangents[i + 1] / s;
      const magSq = a * a + b * b;
      if (magSq > 9) {
        const scale = 3 / Math.sqrt(magSq);
        tangents[i] = scale * a * s;
        tangents[i + 1] = scale * b * s;
      }
    }
  }

  let line = `M ${run[0].x.toFixed(2)} ${run[0].y.toFixed(2)}`;
  for (let i = 0; i < n - 1; i++) {
    const p0 = run[i];
    const p1 = run[i + 1];
    const segDx = dx[i];
    const localMinY = Math.max(topY, Math.min(p0.y, p1.y));
    const localMaxY = Math.min(baseY, Math.max(p0.y, p1.y));
    const cx1 = p0.x + segDx / 3;
    const cy1 = clampValue(p0.y + (tangents[i] * segDx) / 3, localMinY, localMaxY);
    const cx2 = p1.x - segDx / 3;
    const cy2 = clampValue(p1.y - (tangents[i + 1] * segDx) / 3, localMinY, localMaxY);
    line += ` C ${cx1.toFixed(2)} ${cy1.toFixed(2)}, ${cx2.toFixed(2)} ${cy2.toFixed(2)}, ${p1.x.toFixed(2)} ${p1.y.toFixed(2)}`;
  }

  const first = run[0];
  const last = run[n - 1];
  const area = `${line} L ${last.x.toFixed(2)} ${baseY.toFixed(2)} L ${first.x.toFixed(2)} ${baseY.toFixed(2)} Z`;
  return { line, area };
}

export type SparklineChartProps = {
  data: (number | null | undefined)[];
  loading?: boolean;
  color?: string;
  height?: number | string;
  min?: number;
  max?: number;
  autoScale?: boolean;
  showGrid?: boolean;
  showTracks?: boolean;
  barWidth?: number;
  barGap?: number;
  strokeWidth?: number;
  showCurrentDot?: boolean;
  interactive?: boolean;
  activeIndex?: number | null;
  onHoverPoint?: (point: SparklineHoverPoint | null) => void;
  ariaLabel?: string;
  className?: string;
};

function areSparklinePropsEqual(prev: SparklineChartProps, next: SparklineChartProps): boolean {
  if (
    prev.color !== next.color ||
    prev.loading !== next.loading ||
    prev.height !== next.height ||
    prev.min !== next.min ||
    prev.max !== next.max ||
    prev.autoScale !== next.autoScale ||
    prev.showGrid !== next.showGrid ||
    prev.showTracks !== next.showTracks ||
    prev.barWidth !== next.barWidth ||
    prev.barGap !== next.barGap ||
    prev.strokeWidth !== next.strokeWidth ||
    prev.showCurrentDot !== next.showCurrentDot ||
    prev.interactive !== next.interactive ||
    prev.activeIndex !== next.activeIndex ||
    prev.onHoverPoint !== next.onHoverPoint ||
    prev.ariaLabel !== next.ariaLabel ||
    prev.className !== next.className
  ) {
    return false;
  }
  if (prev.data.length !== next.data.length) return false;
  for (let i = 0; i < prev.data.length; i++) {
    if (prev.data[i] !== next.data[i]) return false;
  }
  return true;
}

/**
 * Gráfico vetorial suave de telemetria com preenchimento de área em gradiente
 * semântico, suporte a lacunas (`null`/`NaN`), indicador de leitura atual e
 * crosshair interativo opcional (SVG puro, memoizado, zero cores fixas).
 */
export const SparklineChart = memo(function SparklineChart({
  data,
  loading = false,
  color = "var(--neu-accent)",
  height = 28,
  min = 0,
  max = 100,
  autoScale = false,
  showGrid = false,
  strokeWidth = 1.75,
  showCurrentDot = true,
  interactive = false,
  activeIndex = null,
  onHoverPoint,
  ariaLabel = "Gráfico de telemetria",
  className = "",
}: SparklineChartProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [containerWidth, setContainerWidth] = useState(FALLBACK_WIDTH);
  const [containerHeight, setContainerHeight] = useState(48);
  const rawGradientId = useId();
  const gradientId = useMemo(
    () => `spark-area-${rawGradientId.replace(/[^a-zA-Z0-9_-]/g, "")}`,
    [rawGradientId],
  );

  const numericHeight =
    typeof height === "number" && Number.isFinite(height) && height > 0
      ? height
      : containerHeight > 0
        ? containerHeight
        : 48;
  const chartWidth = containerWidth > 0 ? containerWidth : FALLBACK_WIDTH;

  useEffect(() => {
    const element = containerRef.current;
    if (!element) return;
    const apply = (width: number, measuredHeight: number) => {
      if (width > 0) {
        setContainerWidth(previous => (Math.abs(previous - width) >= 0.5 ? width : previous));
      }
      if (measuredHeight > 0) {
        setContainerHeight(previous =>
          Math.abs(previous - measuredHeight) >= 0.5 ? measuredHeight : previous,
        );
      }
    };
    apply(element.clientWidth, element.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(entries => {
      const entry = entries[0];
      if (entry) apply(entry.contentRect.width, entry.contentRect.height);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const geometry = useMemo(() => {
    const visibleValues =
      data.length > MAX_VISIBLE_POINTS ? data.slice(data.length - MAX_VISIBLE_POINTS) : data;

    let observedPeak = 0;
    const cleanValues: (number | null)[] = new Array(visibleValues.length);
    for (let i = 0; i < visibleValues.length; i++) {
      const v = visibleValues[i];
      if (typeof v === "number" && Number.isFinite(v)) {
        cleanValues[i] = v;
        if (v > observedPeak) observedPeak = v;
      } else {
        cleanValues[i] = null;
      }
    }

    const effectiveMax = autoScale
      ? computeNiceCeiling(observedPeak, max > min ? Math.min(max, 10) : 10)
      : max;
    const range = effectiveMax - min || 1;
    const plotTop = Math.min(PLOT_PAD_TOP, Math.max(2, numericHeight * 0.14));
    const plotBottom = Math.max(
      plotTop + 4,
      numericHeight - Math.min(PLOT_PAD_BOTTOM, Math.max(2, numericHeight * 0.14)),
    );
    const usableHeight = Math.max(4, plotBottom - plotTop);
    const padX = Math.min(PLOT_PAD_X, chartWidth * 0.05);
    const usableWidth = Math.max(4, chartWidth - padX * 2);

    const points: (ChartPoint | null)[] = new Array(cleanValues.length);
    const validPoints: ChartPoint[] = [];

    for (let i = 0; i < cleanValues.length; i++) {
      const rawValue = cleanValues[i];
      if (rawValue == null) {
        points[i] = null;
        continue;
      }
      const clamped = clampValue(rawValue, min, effectiveMax);
      const normalized = (clamped - min) / range;
      const x =
        cleanValues.length <= 1
          ? padX + usableWidth
          : padX + (i / (cleanValues.length - 1)) * usableWidth;
      const y = plotBottom - normalized * usableHeight;
      const pt: ChartPoint = { index: i, value: rawValue, x, y };
      points[i] = pt;
      validPoints.push(pt);
    }

    const runs: ChartPoint[][] = [];
    let currentRun: ChartPoint[] = [];
    for (let i = 0; i < points.length; i++) {
      const pt = points[i];
      if (pt) {
        currentRun.push(pt);
      } else if (currentRun.length > 0) {
        runs.push(currentRun);
        currentRun = [];
      }
    }
    if (currentRun.length > 0) {
      runs.push(currentRun);
    }

    let linePath = "";
    let areaPath = "";
    const isolatedPoints: ChartPoint[] = [];

    for (const run of runs) {
      if (run.length === 1) {
        isolatedPoints.push(run[0]);
      } else {
        const seg = buildMonotoneRunPaths(run, plotBottom, plotTop);
        if (seg.line) linePath += (linePath ? " " : "") + seg.line;
        if (seg.area) areaPath += (areaPath ? " " : "") + seg.area;
      }
    }

    const latestPoint = validPoints.length > 0 ? validPoints[validPoints.length - 1] : null;
    const singlePoint = validPoints.length === 1 ? validPoints[0] : null;

    return {
      plotTop,
      plotBottom,
      padX,
      totalCount: cleanValues.length,
      validPoints,
      isolatedPoints,
      linePath,
      areaPath,
      latestPoint,
      singlePoint,
    };
  }, [data, min, max, autoScale, chartWidth, numericHeight]);

  const activePoint = useMemo(() => {
    if (!interactive || activeIndex == null) return null;
    return geometry.validPoints.find(pt => pt.index === activeIndex) ?? null;
  }, [interactive, activeIndex, geometry.validPoints]);

  const handleMouseMove = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      if (!interactive || !onHoverPoint || geometry.validPoints.length === 0) return;
      const rect = event.currentTarget.getBoundingClientRect();
      const width = rect.width > 0 ? rect.width : chartWidth;
      const relX = clampValue(((event.clientX - rect.left) / width) * chartWidth, 0, chartWidth);
      let nearest = geometry.validPoints[0];
      let bestDist = Math.abs(nearest.x - relX);
      for (let i = 1; i < geometry.validPoints.length; i++) {
        const candidate = geometry.validPoints[i];
        const dist = Math.abs(candidate.x - relX);
        if (dist < bestDist) {
          bestDist = dist;
          nearest = candidate;
        }
      }
      onHoverPoint({
        index: nearest.index,
        value: nearest.value,
        xPercent: (nearest.x / chartWidth) * 100,
        totalCount: geometry.totalCount,
      });
    },
    [interactive, onHoverPoint, geometry.validPoints, geometry.totalCount, chartWidth],
  );

  const handleMouseLeave = useCallback(() => {
    if (interactive && onHoverPoint) {
      onHoverPoint(null);
    }
  }, [interactive, onHoverPoint]);

  const hasSamples = geometry.validPoints.length > 0;
  const midY = (geometry.plotTop + geometry.plotBottom) / 2;

  return (
    <div
      ref={containerRef}
      className={`sparkline-container ${loading ? "chart-is-loading" : ""} ${interactive ? "is-interactive" : ""} ${className}`}
      style={{ height: typeof height === "number" ? `${height}px` : height, width: "100%" }}
      aria-busy={loading}
      onMouseMove={interactive ? handleMouseMove : undefined}
      onMouseLeave={interactive ? handleMouseLeave : undefined}
    >
      {loading && (
        <div
          className="chart-skeleton chart-skeleton-compact"
          role="status"
          aria-label="Coletando dados do gráfico"
        />
      )}
      <svg
        viewBox={`0 0 ${chartWidth} ${numericHeight}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={ariaLabel}
        className="sparkline-svg"
        shapeRendering="geometricPrecision"
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity={0.26} />
            <stop offset="100%" stopColor={color} stopOpacity={0.02} />
          </linearGradient>
        </defs>

        {showGrid && (
          <g className="sparkline-grid" stroke="var(--neu-divider)">
            <line
              x1={geometry.padX}
              y1={geometry.plotTop}
              x2={chartWidth - geometry.padX}
              y2={geometry.plotTop}
              strokeDasharray="3 3"
            />
            <line
              x1={geometry.padX}
              y1={midY}
              x2={chartWidth - geometry.padX}
              y2={midY}
              strokeDasharray="3 3"
            />
            <line
              x1={geometry.padX}
              y1={geometry.plotBottom}
              x2={chartWidth - geometry.padX}
              y2={geometry.plotBottom}
              strokeDasharray="3 3"
            />
          </g>
        )}

        {geometry.areaPath && (
          <path
            d={geometry.areaPath}
            fill={`url(#${gradientId})`}
            stroke="none"
            className="sparkline-area"
          />
        )}

        {geometry.singlePoint && (
          <line
            x1={geometry.padX}
            y1={geometry.singlePoint.y}
            x2={chartWidth - geometry.padX}
            y2={geometry.singlePoint.y}
            stroke={color}
            strokeOpacity="0.38"
            strokeDasharray="4 4"
            strokeWidth={1.25}
            className="sparkline-single-ref"
          />
        )}

        {geometry.linePath && (
          <path
            d={geometry.linePath}
            fill="none"
            stroke={color}
            strokeWidth={strokeWidth}
            strokeLinecap="round"
            strokeLinejoin="round"
            className="sparkline-line"
          />
        )}

        {geometry.isolatedPoints.map(pt => (
          <circle
            key={pt.index}
            cx={pt.x}
            cy={pt.y}
            r={2.5}
            fill={color}
            className="sparkline-point"
          />
        ))}

        {showCurrentDot && geometry.latestPoint && (
          <g className="sparkline-current-indicator">
            <circle
              cx={geometry.latestPoint.x}
              cy={geometry.latestPoint.y}
              r={4.5}
              fill={color}
              fillOpacity="0.22"
              className="sparkline-current-halo"
            />
            <circle
              cx={geometry.latestPoint.x}
              cy={geometry.latestPoint.y}
              r={2.3}
              fill={color}
              stroke="var(--neu-surface)"
              strokeWidth={1.2}
              className="sparkline-current-dot"
            />
          </g>
        )}

        {activePoint && (
          <g className="sparkline-crosshair-group">
            <line
              x1={activePoint.x}
              y1={geometry.plotTop}
              x2={activePoint.x}
              y2={geometry.plotBottom}
              stroke="var(--neu-text-muted)"
              strokeOpacity="0.55"
              strokeDasharray="3 3"
              strokeWidth={1}
              className="sparkline-crosshair"
            />
            <circle
              cx={activePoint.x}
              cy={activePoint.y}
              r={5}
              fill={color}
              fillOpacity="0.24"
              className="sparkline-hover-halo"
            />
            <circle
              cx={activePoint.x}
              cy={activePoint.y}
              r={2.8}
              fill={color}
              stroke="var(--neu-surface)"
              strokeWidth={1.4}
              className="sparkline-hover-dot"
            />
          </g>
        )}
      </svg>
      {!loading && !hasSamples && (
        <span className="chart-unavailable">Indisponível · sem amostras válidas</span>
      )}
    </div>
  );
}, areSparklinePropsEqual);

export type MiniCapacityBarProps = {
  usedGb?: number | null;
  totalGb?: number | null;
  percent?: number | null;
  color?: string;
  ariaLabel?: string;
  loading?: boolean;
};

/**
 * Barra proporcional de capacidade de armazenamento com visual sólido e tokens semânticos.
 */
export const MiniCapacityBar = memo(function MiniCapacityBar({
  usedGb,
  totalGb,
  percent,
  color = "var(--neu-accent)",
  ariaLabel = "Capacidade de armazenamento",
  loading = false,
}: MiniCapacityBarProps) {
  const safePercent =
    typeof percent === "number" && !Number.isNaN(percent)
      ? Math.max(0, Math.min(100, percent))
      : 0;

  const freeGb =
    totalGb != null && usedGb != null && totalGb >= usedGb
      ? Math.max(0, totalGb - usedGb)
      : undefined;

  return (
    <div
      className={`mini-capacity-wrapper ${loading ? "chart-is-loading" : ""}`}
      role="group"
      aria-label={ariaLabel}
      aria-busy={loading}
    >
      {loading ? (
        <div className="capacity-skeleton" role="status" aria-label="Coletando capacidade do disco">
          <span className="capacity-skeleton-fill" aria-hidden="true" />
        </div>
      ) : (
        <>
          <div className="mini-capacity-bar-track">
            <div
              className="mini-capacity-bar-fill"
              style={{
                width: `${safePercent}%`,
                backgroundColor: color,
              }}
            />
          </div>
          <div className="mini-capacity-labels">
            <span>USADO {metric(usedGb, " GB")}</span>
            <span>LIVRE {metric(freeGb, " GB")}</span>
          </div>
        </>
      )}
    </div>
  );
});

export type MiniUsageBarProps = {
  percent?: number | null;
  color?: string;
  ariaLabel?: string;
  leftLabel?: string;
  rightLabel?: string;
};

/**
 * Barra compacta de percentual com legenda própria (ex.: uso da GPU).
 * Sem leitura, o trilho fica hachurado — nunca aparenta 0% medido.
 */
export const MiniUsageBar = memo(function MiniUsageBar({
  percent,
  color = "var(--neu-accent)",
  ariaLabel = "Uso em tempo real",
  leftLabel,
  rightLabel,
}: MiniUsageBarProps) {
  const measured = typeof percent === "number" && Number.isFinite(percent);
  const safePercent = measured ? Math.max(0, Math.min(100, percent)) : 0;

  return (
    <div className="mini-capacity-wrapper" role="group" aria-label={ariaLabel}>
      <div className={`mini-capacity-bar-track${measured ? "" : " is-unavailable"}`}>
        {measured && (
          <div
            className="mini-capacity-bar-fill"
            style={{ width: `${safePercent}%`, backgroundColor: color }}
          />
        )}
      </div>
      {(leftLabel || rightLabel) && (
        <div className="mini-capacity-labels">
          <span>{leftLabel}</span>
          <span>{rightLabel}</span>
        </div>
      )}
    </div>
  );
});

export type DetailedResourceChartProps = {
  title: string;
  history: (number | null | undefined)[];
  currentValue: string;
  subValue?: string;
  color: string;
  ariaLabel?: string;
  windowLabel?: string;
  loading?: boolean;
  unit?: string;
  min?: number;
  max?: number;
  autoScale?: boolean;
  formatValue?: (value: number) => string;
};

/**
 * Painel detalhado de telemetria em tempo real para a aba Desempenho.
 * Superfície recuada (inset well), linha suave com área em gradiente,
 * eixo Y tabular e crosshair interativo ao passar o mouse sem layout shift.
 */
export const DetailedResourceChart = memo(function DetailedResourceChart({
  title,
  history,
  currentValue,
  subValue,
  color,
  ariaLabel,
  windowLabel = "Amostras coletadas",
  loading = false,
  unit = "%",
  min = 0,
  max = 100,
  autoScale = false,
  formatValue,
}: DetailedResourceChartProps) {
  const [hoverPoint, setHoverPoint] = useState<SparklineHoverPoint | null>(null);

  const fmt = useMemo(() => {
    if (formatValue) return formatValue;
    return (value: number) => {
      if (unit === "%") return `${Math.round(value)}%`;
      const formatted =
        Math.abs(value) >= 100
          ? Math.round(value).toLocaleString("pt-BR")
          : (Math.round(value * 10) / 10).toLocaleString("pt-BR");
      return unit ? `${formatted} ${unit}` : formatted;
    };
  }, [formatValue, unit]);

  const { peak, avg, effectiveMax } = useMemo(() => {
    let sum = 0;
    let maxSeen = Number.NEGATIVE_INFINITY;
    let count = 0;
    for (let i = 0; i < history.length; i++) {
      const v = history[i];
      if (typeof v === "number" && Number.isFinite(v)) {
        if (v > maxSeen) maxSeen = v;
        sum += v;
        count++;
      }
    }
    const ceiling = autoScale
      ? computeNiceCeiling(count > 0 ? maxSeen : 0, max > min ? Math.min(max, 10) : 10)
      : max;
    return {
      peak: count > 0 ? fmt(maxSeen) : "Indisponível",
      avg: count > 0 ? fmt(sum / count) : "Indisponível",
      effectiveMax: ceiling,
    };
  }, [history, autoScale, max, min, fmt]);

  const yMid = min + (effectiveMax - min) / 2;

  return (
    <div
      className={`detailed-chart-panel ${loading ? "chart-is-loading" : ""}`}
      role="region"
      aria-label={ariaLabel || title}
      aria-busy={loading}
    >
      <div className="detailed-chart-header">
        <div className="detailed-chart-title-box">
          <span className="detailed-chart-title">{title}</span>
          {subValue && <span className="detailed-chart-sub">{subValue}</span>}
        </div>
        <div className={`detailed-chart-stats ${loading ? "chart-loading-stats" : ""}`}>
          <span className="stat-badge stat-current" style={{ borderColor: color, color }}>
            ATUAL: <strong>{currentValue}</strong>
          </span>
          <span className="stat-badge">
            PICO: <strong>{peak}</strong>
          </span>
          <span className="stat-badge">
            MÉDIA: <strong>{avg}</strong>
          </span>
        </div>
      </div>

      <div className="detailed-chart-body">
        <div className="detailed-chart-y-axis">
          <span>{fmt(effectiveMax)}</span>
          <span>{fmt(yMid)}</span>
          <span>{fmt(min)}</span>
        </div>

        <div className="detailed-chart-plot">
          {loading ? (
            <div
              className="chart-skeleton chart-skeleton-detailed"
              role="status"
              aria-label={`Coletando ${title.toLocaleLowerCase("pt-BR")}`}
            >
              <span className="chart-skeleton-grid" aria-hidden="true" />
              <span className="chart-skeleton-caption">COLETANDO PRIMEIRAS AMOSTRAS</span>
            </div>
          ) : (
            <>
              <SparklineChart
                data={history}
                color={color}
                min={min}
                max={effectiveMax}
                height="100%"
                showGrid
                strokeWidth={2}
                interactive
                activeIndex={hoverPoint?.index ?? null}
                onHoverPoint={setHoverPoint}
                ariaLabel={ariaLabel || title}
              />
              {hoverPoint && (
                <div
                  className={`detailed-chart-tooltip ${hoverPoint.xPercent > 55 ? "align-left" : "align-right"}`}
                  role="status"
                >
                  <strong style={{ color }}>{fmt(hoverPoint.value)}</strong>
                  <span>
                    Amostra {hoverPoint.index + 1}/{hoverPoint.totalCount}
                  </span>
                </div>
              )}
            </>
          )}
        </div>
      </div>

      <div className="detailed-chart-timeline">
        <span>Primeira amostra</span>
        <span className="timeline-window-label">{windowLabel}</span>
        <span>Agora</span>
      </div>
    </div>
  );
});

export type HardwareGaugeBarProps = {
  label: string;
  valueText: string;
  percent?: number | null;
  statusText?: string | null;
  type?: "temperature" | "vram" | "gpu";
};

/**
 * Medidor visual de hardware para controladora de vídeo, VRAM e sensores térmicos.
 */
export const HardwareGaugeBar = memo(function HardwareGaugeBar({
  label,
  valueText,
  percent,
  statusText,
  type = "gpu",
}: HardwareGaugeBarProps) {
  const safePercent =
    typeof percent === "number" && !Number.isNaN(percent)
      ? Math.max(0, Math.min(100, percent))
      : undefined;

  let barFill = "var(--neu-accent)";
  if (type === "temperature") {
    barFill =
      safePercent !== undefined && safePercent >= 80
        ? "var(--neu-danger)"
        : safePercent !== undefined && safePercent >= 65
          ? "var(--neu-warning)"
          : "var(--neu-signal-secondary)";
  } else if (type === "vram") {
    barFill = "var(--neu-info)";
  }

  return (
    <div className="hw-gauge-card">
      <div className="hw-gauge-head">
        <span className="hw-gauge-label">{label}</span>
        <strong className="hw-gauge-value">{valueText}</strong>
      </div>

      {safePercent !== undefined ? (
        <div className="hw-gauge-bar-track">
          <div
            className="hw-gauge-bar-fill"
            style={{
              width: `${safePercent}%`,
              backgroundColor: barFill,
            }}
          />
        </div>
      ) : (
        <div className="hw-gauge-bar-track hw-track-disabled">
          <div className="hw-gauge-bar-fill hw-fill-empty" />
        </div>
      )}

      {statusText && <small className="hw-gauge-status">{statusText}</small>}
    </div>
  );
});
