import { memo, useEffect, useId, useMemo, useRef, useState } from "react";
import { metric } from "./App";

const FALLBACK_WIDTH = 320;
const TOP_PADDING = 2;

export type SparklineChartProps = {
  data: (number | null | undefined)[];
  loading?: boolean;
  color?: string;
  height?: number | string;
  min?: number;
  max?: number;
  showGrid?: boolean;
  barWidth?: number;
  barGap?: number;
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
    prev.showGrid !== next.showGrid ||
    prev.barWidth !== next.barWidth ||
    prev.barGap !== next.barGap ||
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
 * Medidor de telemetria em barras verticais (SVG puro, sem dependências, memoizado).
 * As colunas usam largura e espaçamento fixos em pixels reais (medidos via ResizeObserver)
 * e são ancoradas na borda direita: o histórico cresce para a esquerda, como um registrador
 * em rolagem. O gradiente horizontal sinaliza a idade do dado — a amostra mais recente,
 * à direita, mantém a cor cheia; as antigas esmaecem. Um único <path> evita dezenas de
 * nós DOM no histórico completo.
 */
export const SparklineChart = memo(function SparklineChart({
  data,
  loading = false,
  color = "#8da9bf",
  height = 28,
  min = 0,
  max = 100,
  showGrid = false,
  barWidth = 4,
  barGap = 2,
  ariaLabel = "Gráfico de telemetria",
  className = "",
}: SparklineChartProps) {
  const rawId = useId();
  const gradientId = `spark-fade-${rawId.replace(/[^a-zA-Z0-9-_]/g, "")}`;
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [containerWidth, setContainerWidth] = useState(FALLBACK_WIDTH);

  const numericHeight =
    typeof height === "number" && Number.isFinite(height) && height > 0 ? height : 40;
  const chartWidth = containerWidth > 0 ? containerWidth : FALLBACK_WIDTH;

  useEffect(() => {
    const element = containerRef.current;
    if (!element) return;
    const apply = (value: number) => {
      if (value <= 0) return;
      setContainerWidth(previous => (Math.abs(previous - value) >= 0.5 ? value : previous));
    };
    apply(element.clientWidth);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(entries => {
      const entry = entries[0];
      if (entry) apply(entry.contentRect.width);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const { barsPath, barThickness, hasSamples, occupiedStart } = useMemo(() => {
    // Sanitiza dados contra null, undefined e NaN
    const sanitized: (number | null)[] = [];
    for (let i = 0; i < data.length; i++) {
      const v = data[i];
      sanitized.push(
        typeof v === "number" && Number.isFinite(v) ? Math.max(min, Math.min(max, v)) : null,
      );
    }

    const len = sanitized.length;
    if (len === 0) {
      return { barsPath: "", barThickness: barWidth, hasSamples: false, occupiedStart: chartWidth };
    }

    const range = max - min || 1;
    const usableHeight = numericHeight - TOP_PADDING;
    const slot = Math.min(barWidth + barGap, chartWidth / len);
    const thickness = Math.max(0.5, Math.min(barWidth, slot * 0.78));
    const occupiedWidth = slot * len;
    const startX = chartWidth - occupiedWidth;

    let path = "";
    let hasSamples = false;
    for (let i = 0; i < len; i++) {
      const value = sanitized[i];
      if (value == null) continue;
      hasSamples = true;
      const centerX = startX + slot * (i + 0.5);
      const normalized = (value - min) / range;
      const topY = numericHeight - normalized * usableHeight;
      path += ` M ${centerX.toFixed(2)} ${numericHeight} L ${centerX.toFixed(2)} ${topY.toFixed(2)}`;
    }

    return { barsPath: path.trim(), barThickness: thickness, hasSamples, occupiedStart: startX };
  }, [data, min, max, chartWidth, numericHeight, barWidth, barGap]);

  return (
    <div
      ref={containerRef}
      className={`sparkline-container ${loading ? "chart-is-loading" : ""} ${className}`}
      style={{ height: typeof height === "number" ? `${height}px` : height, width: "100%" }}
      aria-busy={loading}
    >
      {loading && (
        <div className="chart-skeleton chart-skeleton-compact" role="status" aria-label="Coletando dados do gráfico" />
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
          <linearGradient
            id={gradientId}
            x1={occupiedStart}
            y1="0"
            x2={chartWidth}
            y2="0"
            gradientUnits="userSpaceOnUse"
          >
            <stop offset="0%" stopColor={color} stopOpacity="0.2" />
            <stop offset="100%" stopColor={color} stopOpacity="1" />
          </linearGradient>
        </defs>

        {showGrid && (
          <g className="sparkline-grid" stroke="rgba(255, 255, 255, 0.07)">
            <line x1="0" y1={TOP_PADDING} x2={chartWidth} y2={TOP_PADDING} />
            <line x1="0" y1={(numericHeight + TOP_PADDING) / 2} x2={chartWidth} y2={(numericHeight + TOP_PADDING) / 2} />
            <line x1="0" y1={numericHeight} x2={chartWidth} y2={numericHeight} />
          </g>
        )}

        {barsPath && <path
          d={barsPath}
          fill="none"
          stroke={`url(#${gradientId})`}
          strokeWidth={barThickness}
          strokeLinecap="butt"
          className="sparkline-bars"
        />}
      </svg>
      {!loading && !hasSamples && <span className="chart-unavailable">Indisponível · sem amostras válidas</span>}
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
  color = "#8da9bf",
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
            /* O gráfico acompanha a altura disponível do card para que a
               grade de Desempenho caiba alinhada em janelas mais baixas. */
            height="100%"
            showGrid
            barWidth={6}
            barGap={3}
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
