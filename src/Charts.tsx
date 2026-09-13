import { memo, useId, useMemo } from "react";
import { metric } from "./App";

export type SparklineChartProps = {
  data: (number | null | undefined)[];
  loading?: boolean;
  color?: string;
  height?: number | string;
  min?: number;
  max?: number;
  showArea?: boolean;
  showPoint?: boolean;
  showGrid?: boolean;
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
    prev.showArea !== next.showArea ||
    prev.showPoint !== next.showPoint ||
    prev.showGrid !== next.showGrid ||
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
 * Gráfico Sparkline vetorial ultra-otimizado (SVG puro, sem dependências, memoizado).
 * Utiliza interpolação cúbica Bézier monotônica suave que nunca ultrapassa os limites dos dados.
 */
export const SparklineChart = memo(function SparklineChart({
  data,
  loading = false,
  color = "#8da9bf",
  height = 28,
  min = 0,
  max = 100,
  showArea = true,
  showPoint = true,
  showGrid = false,
  ariaLabel = "Gráfico de telemetria",
  className = "",
}: SparklineChartProps) {
  const rawId = useId();
  const gradientId = `spark-grad-${rawId.replace(/[^a-zA-Z0-9-_]/g, "")}`;

  const width = 100;
  const chartHeight = 40;
  const paddingY = 3;

  const { linePath, areaPath, lastCoord } = useMemo(() => {
    // Sanitiza dados contra null, undefined e NaN
    const sanitized: (number | null)[] = [];
    for (let i = 0; i < data.length; i++) {
      const v = data[i];
      sanitized.push(
        typeof v === "number" && Number.isFinite(v) ? Math.max(min, Math.min(max, v)) : null,
      );
    }

    const points = sanitized;

    const effectiveHeight = chartHeight - paddingY * 2;
    const range = max - min || 1;
    const len = points.length;

    const coords: ({ x: number; y: number } | null)[] = new Array(len);
    for (let i = 0; i < len; i++) {
      const value = points[i];
      if (value == null) { coords[i] = null; continue; }
      const x = len === 1 ? width / 2 : (i / (len - 1)) * width;
      const normalized = (value - min) / range;
      const y = chartHeight - paddingY - normalized * effectiveHeight;
      coords[i] = { x, y };
    }

    // Constrói caminho Bézier cúbico contínuo
    let line = "";
    let area = "";
    let first: { x: number; y: number } | null = null;
    for (let i = 0; i < len; i++) {
      const prev = coords[i - 1];
      const curr = coords[i];
      if (!curr) continue;
      let segment: string;
      if (!prev) {
        first = curr;
        segment = `M ${curr.x.toFixed(2)} ${curr.y.toFixed(2)}`;
      } else {
        const midX = (prev.x + curr.x) / 2;
        segment = ` C ${midX.toFixed(2)} ${prev.y.toFixed(2)}, ${midX.toFixed(2)} ${curr.y.toFixed(2)}, ${curr.x.toFixed(2)} ${curr.y.toFixed(2)}`;
      }
      line += segment;
      area += segment;
      if (!coords[i + 1] && first) area += ` L ${curr.x} ${chartHeight} L ${first.x} ${chartHeight} Z `;
    }

    const last = coords[len - 1];

    return { linePath: line, areaPath: area, lastCoord: last };
  }, [data, min, max]);

  return (
    <div
      className={`sparkline-container ${loading ? "chart-is-loading" : ""} ${className}`}
      style={{ height: typeof height === "number" ? `${height}px` : height, width: "100%" }}
      aria-busy={loading}
    >
      {loading && (
        <div className="chart-skeleton chart-skeleton-compact" role="status" aria-label="Coletando dados do gráfico" />
      )}
      <svg
        viewBox={`0 0 ${width} ${chartHeight}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={ariaLabel}
        className="sparkline-svg"
        shapeRendering="geometricPrecision"
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity="0.35" />
            <stop offset="100%" stopColor={color} stopOpacity="0.02" />
          </linearGradient>
        </defs>

        {showGrid && (
          <g className="sparkline-grid" stroke="rgba(255, 255, 255, 0.08)" strokeDasharray="2 3">
            <line x1="0" y1={paddingY} x2={width} y2={paddingY} />
            <line x1="0" y1={chartHeight / 2} x2={width} y2={chartHeight / 2} />
            <line x1="0" y1={chartHeight - paddingY} x2={width} y2={chartHeight - paddingY} />
          </g>
        )}

        {showArea && areaPath && (
          <path
            d={areaPath}
            fill={`url(#${gradientId})`}
            className="sparkline-area"
          />
        )}

        {linePath && <path
          d={linePath}
          fill="none"
          stroke={color}
          strokeWidth="1.75"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="sparkline-line"
        />}

        {showPoint && lastCoord && (
          <g className="sparkline-marker">
            <circle
              cx={lastCoord.x}
              cy={lastCoord.y}
              r="4.5"
              fill={color}
              fillOpacity="0.25"
              className="sparkline-marker-halo"
            />
            <circle
              cx={lastCoord.x}
              cy={lastCoord.y}
              r="2.5"
              fill={color}
              className="sparkline-marker-dot"
            />
          </g>
        )}
      </svg>
      {!loading && !linePath && <span className="chart-unavailable">Indisponível · sem amostras válidas</span>}
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
 * Barra proporcional de capacidade de armazenamento com visual segmentado de alta precisão.
 */
export const MiniCapacityBar = memo(function MiniCapacityBar({
  usedGb,
  totalGb,
  percent,
  color = "#8da9bf",
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
    <div className={`mini-capacity-wrapper ${loading ? "chart-is-loading" : ""}`} role="group" aria-label={ariaLabel} aria-busy={loading}>
      {loading ? (
        <div className="capacity-skeleton" role="status" aria-label="Coletando capacidade do disco">
          <span className="capacity-skeleton-fill" aria-hidden="true" />
        </div>
      ) : <>
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
      </>}
    </div>
  );
});

export type DetailedResourceChartProps = {
  title: string;
  history: number[];
  currentValue: string;
  subValue?: string;
  color: string;
  ariaLabel?: string;
  windowLabel?: string;
  loading?: boolean;
};

/**
 * Painel detalhado de telemetria em tempo real para a aba Desempenho.
 * Exibe gráfico vetorial com eixos Y (0%, 50%, 100%), tempo no eixo X e badges de Pico e Média.
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
}: DetailedResourceChartProps) {
  const { peak, avg } = useMemo(() => {
    let sum = 0;
    let max = 0;
    let count = 0;
    for (let i = 0; i < history.length; i++) {
      const v = history[i];
      if (typeof v === "number" && !Number.isNaN(v)) {
        if (v > max) max = v;
        sum += v;
        count++;
      }
    }
    return {
      peak: count > 0 ? `${Math.round(max)}%` : "Indisponível",
      avg: count > 0 ? `${Math.round(sum / count)}%` : "Indisponível",
    };
  }, [history]);

  return (
    <div className={`detailed-chart-panel ${loading ? "chart-is-loading" : ""}`} role="region" aria-label={ariaLabel || title} aria-busy={loading}>
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
          <span>100%</span>
          <span>50%</span>
          <span>0%</span>
        </div>

        <div className="detailed-chart-plot">
          {loading ? (
            <div className="chart-skeleton chart-skeleton-detailed" role="status" aria-label={`Coletando ${title.toLocaleLowerCase("pt-BR")}`}>
              <span className="chart-skeleton-grid" aria-hidden="true" />
              <span className="chart-skeleton-caption">COLETANDO PRIMEIRAS AMOSTRAS</span>
            </div>
          ) : <SparklineChart
            data={history}
            color={color}
            height={90}
            showArea
            showPoint
            showGrid
            ariaLabel={ariaLabel || title}
          />}
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

  let barGradient = "linear-gradient(90deg, #8da9bf, #b4bdc6)";
  if (type === "temperature") {
    barGradient = "linear-gradient(90deg, #8da9bf 0%, #8da9bf 45%, #f59e0b 75%, #ef4444 100%)";
  } else if (type === "vram") {
    barGradient = "linear-gradient(90deg, #8da9bf, #b4bdc6)";
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
              background: barGradient,
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
