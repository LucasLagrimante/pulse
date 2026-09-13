import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  DetailedResourceChart,
  HardwareGaugeBar,
  MiniCapacityBar,
  SparklineChart,
} from "./Charts";

describe("SparklineChart component", () => {
  it("shows a transient skeleton without claiming data is unavailable", () => {
    render(<SparklineChart data={[]} loading ariaLabel="Loading CPU chart" />);
    expect(screen.getByRole("status", { name: "Coletando dados do gráfico" })).toBeInTheDocument();
    expect(screen.queryByText(/sem amostras válidas/)).not.toBeInTheDocument();
  });
  it("does not draw zero for an unavailable series", () => {
    const { container } = render(<SparklineChart data={[null, undefined, NaN, Infinity]} />);
    expect(container.querySelector('.sparkline-line')).not.toBeInTheDocument();
    expect(screen.getByText(/sem amostras válidas/)).toBeInTheDocument();
  });

  it("leaves a gap between measurements separated by missing data", () => {
    const { container } = render(<SparklineChart data={[20, null, 40]} />);
    const path = container.querySelector('.sparkline-line')?.getAttribute('d');
    expect(path?.match(/M/g)).toHaveLength(2);
    expect(path).not.toContain(' C ');
  });
  it("renders with valid data, paths and accessible attributes", () => {
    const { container } = render(
      <SparklineChart
        data={[10, 25, 40, 30, 80]}
        color="#d95d39"
        height={32}
        showArea
        showPoint
        showGrid
        ariaLabel="CPU Activity Sparkline"
      />,
    );

    const svg = screen.getByRole("img", { name: "CPU Activity Sparkline" });
    expect(svg).toBeInTheDocument();
    expect(svg.querySelector("path.sparkline-line")).toBeInTheDocument();
    expect(svg.querySelector("path.sparkline-area")).toBeInTheDocument();
    expect(container.querySelector(".sparkline-marker")).toBeInTheDocument();
    expect(container.querySelector(".sparkline-grid")).toBeInTheDocument();
  });

  it("handles empty arrays and single points gracefully without crashing", () => {
    const { rerender } = render(<SparklineChart data={[]} ariaLabel="Empty chart" />);
    expect(screen.getByRole("img", { name: "Empty chart" })).toBeInTheDocument();

    rerender(<SparklineChart data={[50]} ariaLabel="Single point chart" />);
    expect(screen.getByRole("img", { name: "Single point chart" })).toBeInTheDocument();
  });

  it("handles null, undefined and NaN values safely", () => {
    render(
      <SparklineChart
        data={[null, undefined, Number.NaN, 20, null, 70] as unknown as number[]}
        ariaLabel="Resilient chart"
      />,
    );
    expect(screen.getByRole("img", { name: "Resilient chart" })).toBeInTheDocument();
  });
});

describe("MiniCapacityBar component", () => {
  it("renders used and free labels and bounds progress bar accurately", () => {
    render(
      <MiniCapacityBar
        usedGb={150}
        totalGb={500}
        percent={30}
        color="#38bdf8"
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
        color="#dda76d"
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
        color="#d95d39"
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
});

describe("HardwareGaugeBar component", () => {
  it("renders status, value text and gradient bar", () => {
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
