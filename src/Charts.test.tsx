import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  DetailedResourceChart,
  HardwareGaugeBar,
  MiniCapacityBar,
  SparklineChart,
  computeNiceCeiling,
} from "./Charts";

describe("SparklineChart component", () => {
  it("shows a transient skeleton without claiming data is unavailable", () => {
    render(<SparklineChart data={[]} loading ariaLabel="Loading CPU chart" />);
    expect(screen.getByRole("status", { name: "Coletando dados do gráfico" })).toBeInTheDocument();
    expect(screen.queryByText(/sem amostras válidas/)).not.toBeInTheDocument();
  });

  it("does not draw zero for an unavailable series", () => {
    const { container } = render(<SparklineChart data={[null, undefined, NaN, Infinity]} />);
    expect(container.querySelector(".sparkline-line")).not.toBeInTheDocument();
    expect(container.querySelector(".sparkline-area")).not.toBeInTheDocument();
    expect(container.querySelector(".sparkline-current-dot")).not.toBeInTheDocument();
    expect(screen.getByText(/sem amostras válidas/)).toBeInTheDocument();
  });

  it("breaks continuous line across missing data gaps", () => {
    const { container } = render(<SparklineChart data={[20, null, 40]} />);
    expect(container.querySelector(".sparkline-line")).not.toBeInTheDocument();
    expect(container.querySelectorAll(".sparkline-point")).toHaveLength(2);
    expect(container.querySelector(".sparkline-current-dot")).toBeInTheDocument();
  });

  it("renders smooth vector curve, vertical area gradient, dashed grid and current reading indicator", () => {
    render(
      <SparklineChart
        data={[10, 25, 40, 30, 80]}
        color="var(--neu-accent)"
        height={32}
        showGrid
        ariaLabel="CPU Activity Sparkline"
      />,
    );

    const svg = screen.getByRole("img", { name: "CPU Activity Sparkline" });
    expect(svg).toBeInTheDocument();

    const grad = svg.querySelector("defs linearGradient");
    expect(grad).toBeInTheDocument();
    const stops = grad?.querySelectorAll("stop") ?? [];
    expect(stops).toHaveLength(2);
    expect(stops[0].getAttribute("stop-color")).toBe("var(--neu-accent)");

    const line = svg.querySelector("path.sparkline-line");
    const area = svg.querySelector("path.sparkline-area");
    expect(line).toBeInTheDocument();
    expect(line?.getAttribute("d")).toContain(" C ");
    expect(line?.getAttribute("stroke")).toBe("var(--neu-accent)");
    expect(area).toBeInTheDocument();
    expect(area?.getAttribute("fill")).toMatch(/^url\(#spark-area-/);

    const gridLines = svg.querySelectorAll(".sparkline-grid line");
    expect(gridLines).toHaveLength(3);
    expect(svg.querySelector(".sparkline-current-halo")).toBeInTheDocument();
    expect(svg.querySelector(".sparkline-current-dot")).toBeInTheDocument();
  });

  it("renders reference line and sample dot when there is only 1 valid sample", () => {
    const { container } = render(<SparklineChart data={[50]} ariaLabel="Single point chart" />);
    expect(screen.getByRole("img", { name: "Single point chart" })).toBeInTheDocument();
    expect(container.querySelector(".sparkline-single-ref")).toBeInTheDocument();
    expect(container.querySelector(".sparkline-current-dot")).toBeInTheDocument();
    expect(container.querySelector(".sparkline-line")).not.toBeInTheDocument();
  });

  it("handles empty arrays and null/NaN values safely without crashing", () => {
    const { rerender } = render(<SparklineChart data={[]} ariaLabel="Empty chart" />);
    expect(screen.getByRole("img", { name: "Empty chart" })).toBeInTheDocument();

    rerender(
      <SparklineChart
        data={[null, undefined, Number.NaN, 20, 35, null, 70, 82] as unknown as number[]}
        ariaLabel="Resilient chart"
      />,
    );
    expect(screen.getByRole("img", { name: "Resilient chart" })).toBeInTheDocument();
  });

  it("computes clean auto-scale ceilings for open-ended metrics", () => {
    expect(computeNiceCeiling(0, 10)).toBe(10);
    expect(computeNiceCeiling(140, 10)).toBe(200);
    expect(computeNiceCeiling(1850, 100)).toBe(2500);
  });
});

describe("MiniCapacityBar component", () => {
  it("renders used and free labels and bounds progress bar accurately", () => {
    render(
      <MiniCapacityBar
        usedGb={150}
        totalGb={500}
        percent={30}
        color="var(--neu-info)"
        ariaLabel="Uso de disco"
      />,
    );

    expect(screen.getByRole("group", { name: "Uso de disco" })).toBeInTheDocument();
    expect(screen.getByText(/USADO 150 GB/)).toBeInTheDocument();
    expect(screen.getByText(/LIVRE 350 GB/)).toBeInTheDocument();
  });

  it("handles null and undefined safely", () => {
    render(<MiniCapacityBar usedGb={null} totalGb={null} percent={null} />);
    expect(screen.getByText(/USADO —/)).toBeInTheDocument();
    expect(screen.getByText(/LIVRE —/)).toBeInTheDocument();
  });
});

describe("DetailedResourceChart component", () => {
  it("shows the first-sample skeleton while telemetry is loading", () => {
    render(
      <DetailedResourceChart
        title="USO DA CPU EM TEMPO REAL"
        history={[]}
        currentValue="—"
        color="var(--neu-accent)"
        loading
      />,
    );
    expect(screen.getByRole("region")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("COLETANDO PRIMEIRAS AMOSTRAS")).toBeInTheDocument();
  });

  it("calculates peak, average and renders timeline labels", () => {
    render(
      <DetailedResourceChart
        title="CARGA DA CPU EM TEMPO REAL"
        history={[10, 20, 50, 40]}
        currentValue="40,0%"
        color="var(--neu-accent)"
        ariaLabel="Painel detalhado de CPU"
      />,
    );

    expect(screen.getByRole("region", { name: "Painel detalhado de CPU" })).toBeInTheDocument();
    expect(screen.getByText("CARGA DA CPU EM TEMPO REAL")).toBeInTheDocument();
    expect(screen.getByText("40,0%")).toBeInTheDocument();
    expect(screen.getByText(/PICO:/)).toHaveTextContent("50%");
    expect(screen.getByText("Primeira amostra")).toBeInTheDocument();
    expect(screen.getByText("Agora")).toBeInTheDocument();
  });

  it("shows interactive crosshair and floating sample tooltip on hover without layout shift", () => {
    const { container } = render(
      <DetailedResourceChart
        title="CARGA DA CPU EM TEMPO REAL"
        history={[10, 25, 65, 40]}
        currentValue="40%"
        color="var(--neu-accent)"
        ariaLabel="Painel interativo"
      />,
    );

    const sparkWrap = container.querySelector(".sparkline-container") as HTMLElement;
    expect(sparkWrap).toBeInTheDocument();
    sparkWrap.getBoundingClientRect = () =>
      ({
        left: 0,
        top: 0,
        width: 300,
        height: 100,
        right: 300,
        bottom: 100,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      }) as DOMRect;

    fireEvent.mouseMove(sparkWrap, { clientX: 200 });
    expect(container.querySelector(".sparkline-crosshair")).toBeInTheDocument();
    expect(container.querySelector(".sparkline-hover-dot")).toBeInTheDocument();
    const tooltip = container.querySelector(".detailed-chart-tooltip");
    expect(tooltip).toBeInTheDocument();
    expect(tooltip).toHaveTextContent("65%");
    expect(tooltip).toHaveTextContent("Amostra 3/4");

    fireEvent.mouseLeave(sparkWrap);
    expect(container.querySelector(".sparkline-crosshair")).not.toBeInTheDocument();
    expect(container.querySelector(".detailed-chart-tooltip")).not.toBeInTheDocument();
  });

  it("supports custom units and autoScale for non-percentage telemetry", () => {
    render(
      <DetailedResourceChart
        title="ROTAÇÃO DA VENTOINHA"
        history={[900, 1200, 1500]}
        currentValue="1.500 RPM"
        color="var(--neu-info)"
        unit="RPM"
        autoScale
        max={1000}
      />,
    );
    expect(screen.getByText(/PICO:/)).toHaveTextContent("1.500 RPM");
    expect(screen.getByText(/MÉDIA:/)).toHaveTextContent("1.200 RPM");
  });
});

describe("HardwareGaugeBar component", () => {
  it("renders status, value text and semantic bar", () => {
    render(
      <HardwareGaugeBar
        label="TEMPERATURA"
        valueText="45,5 °C"
        percent={45.5}
        statusText="Operação ideal"
        type="temperature"
      />,
    );

    expect(screen.getByText("TEMPERATURA")).toBeInTheDocument();
    expect(screen.getByText("45,5 °C")).toBeInTheDocument();
    expect(screen.getByText("Operação ideal")).toBeInTheDocument();
  });

  it("renders disabled state safely when percent is missing", () => {
    const { container } = render(
      <HardwareGaugeBar
        label="VRAM DEDICADA"
        valueText="—"
        percent={null}
        statusText="VRAM não exposta pelo driver"
        type="vram"
      />,
    );

    expect(screen.getByText("VRAM DEDICADA")).toBeInTheDocument();
    expect(screen.getByText("VRAM não exposta pelo driver")).toBeInTheDocument();
    expect(container.querySelector(".hw-track-disabled")).toBeInTheDocument();
  });
});
