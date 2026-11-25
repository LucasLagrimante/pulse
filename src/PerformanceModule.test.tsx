import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { PerformanceModule } from "./PerformanceModule";
import {
  DEFAULT_TELEMETRY_PREFERENCES,
  buildUnifiedMetrics,
  sanitizeTelemetryPreferences,
  type HardwareTelemetrySnapshot,
  type PerformanceGroup,
  type TelemetryPreferences,
} from "./telemetry";

const sampleSnapshot: HardwareTelemetrySnapshot = {
  cpu: {
    model: "AMD Ryzen 7 7800X3D",
    physicalCores: 8,
    logicalCores: 16,
    baseClockMhz: 4200,
    currentClockMhz: 4850,
    perCorePercent: [18, 24, 12, 30, 15, 22, 10, 41],
    uptimeSeconds: 95400,
    processCount: 214,
    threadCount: 3120,
    handleCount: 98400,
  },
  ram: {
    usedGb: 14.2,
    totalGb: 32,
    availableGb: 17.8,
    cachedGb: 8.4,
    commitUsedGb: 18.6,
    commitLimitGb: 40,
    speedMhz: 6000,
    slotsUsed: 2,
  },
  gpu: {
    name: "NVIDIA GeForce RTX 4070 SUPER",
    usagePercent: 44,
    vramUsedGb: 4.5,
    vramTotalGb: 12,
    sharedUsedGb: 0.4,
    temperatureCelsius: 58,
    fanPercent: 42,
    fanRpm: null,
    coreClockMhz: 2490,
    memoryClockMhz: 10501,
    powerWatts: 132.4,
    powerLimitWatts: 220,
    source: "DXGI + nvidia-smi",
  },
  diskIo: {
    readMbps: 84.2,
    writeMbps: 31.5,
    activePercent: 14,
    queueLength: 0.3,
    readIops: 420,
    writeIops: 180,
  },
  metrics: [
    {
      id: "cpu.clock_ghz",
      group: "cpu",
      label: "Frequência da CPU",
      shortLabel: "CLOCK CPU",
      value: 4.85,
      unit: "GHz",
      kind: "clock",
      maxValue: 6.0,
      detail: "Base 4,20 GHz",
      source: "PDH",
      unavailableReason: null,
    },
    {
      id: "cpu.temp",
      group: "cpu",
      label: "Temperatura CPU (CPU Package)",
      shortLabel: "TEMP CPU",
      value: 56.4,
      unit: "°C",
      kind: "temperature",
      maxValue: 100,
      detail: "LibreHardwareMonitor",
      source: "LibreHardwareMonitor",
      unavailableReason: null,
    },
    {
      id: "gpu.fan_percent",
      group: "gpu",
      label: "Ventoinha da GPU",
      shortLabel: "FAN GPU",
      value: 42,
      unit: "%",
      kind: "percent",
      maxValue: 100,
      detail: null,
      source: "nvidia-smi",
      unavailableReason: null,
    },
    {
      id: "sensor.lhm-fan-cpu",
      group: "sensors",
      label: "CPU Fan",
      shortLabel: "CPU FAN",
      value: 1240,
      unit: "RPM",
      kind: "fan",
      maxValue: null,
      detail: "LibreHardwareMonitor",
      source: "LibreHardwareMonitor",
      unavailableReason: null,
    },
  ],
  externalProviders: ["LibreHardwareMonitor"],
  sampledAtUnixMs: 1_700_000_000_000,
};

function Harness({
  initialPrefs = DEFAULT_TELEMETRY_PREFERENCES,
  snapshot = sampleSnapshot,
  onPrefsChanged,
}: {
  initialPrefs?: TelemetryPreferences;
  snapshot?: HardwareTelemetrySnapshot;
  onPrefsChanged?: (next: TelemetryPreferences) => void;
}) {
  const [group, setGroup] = useState<PerformanceGroup | null>(null);
  const [prefs, setPrefs] = useState<TelemetryPreferences>(initialPrefs);
  const unified = buildUnifiedMetrics({
    cpuPercent: 28,
    memoryPercent: 44,
    memoryUsedGb: 14.2,
    memoryTotalGb: 32,
    diskPercent: 52,
    diskUsedGb: 520,
    diskTotalGb: 1000,
    gpuPercent: snapshot.gpu.usagePercent ?? 44,
    gpuName: snapshot.gpu.name ?? "NVIDIA GeForce RTX 4070 SUPER",
    downloadBps: 1_500_000,
    uploadBps: 250_000,
    hardwareTelemetry: snapshot,
  });

  const handleUpdatePrefs = (next: TelemetryPreferences) => {
    setPrefs(next);
    onPrefsChanged?.(next);
  };

  return (
    <PerformanceModule
      overview={{
        cpuPercent: 28,
        gpuPercent: snapshot.gpu.usagePercent ?? 44,
        memoryUsedGb: 14.2,
        memoryTotalGb: 32,
        diskUsedGb: 520,
        diskTotalGb: 1000,
      }}
      performance={{ gpu: { value: snapshot.gpu.name ?? "GPU" }, thermalSensors: [] }}
      devices={{
        adapters: { value: [{ name: "Ethernet", status: "Ativo", receivedBps: { value: 1_500_000 }, sentBps: { value: 250_000 } }] },
        disks: {
          value: [
            {
              index: 0,
              model: "Samsung 990 PRO 2TB",
              mediaType: "SSD",
              busType: "NVMe",
              sizeBytes: { value: 2_000_000_000_000 },
              health: { value: "Saudável" },
              temperatureCelsius: { value: 41 },
              wearPercent: 3,
              powerOnHours: 1240,
            },
          ],
        },
        volumes: [{ path: "C:\\", label: "Disco C:", totalBytes: 1_000_000_000_000, freeBytes: 480_000_000_000 }],
      }}
      hardwareTelemetry={snapshot}
      unifiedMetrics={unified}
      metricHistories={{ "cpu.clock_ghz": [4.5, 4.7, 4.85], "cpu.temp": [54, 55, 56.4] }}
      preferences={prefs}
      onUpdatePreferences={handleUpdatePrefs}
      selectedGroup={group}
      onSelectGroup={setGroup}
      cpuHistory={[20, 25, 28]}
      ramHistory={[42, 43, 44]}
      gpuHistory={[38, 41, 44]}
      downloadHistory={[1_000_000, 1_500_000]}
      uploadHistory={[200_000, 250_000]}
      cpuChartLoading={false}
      ramChartLoading={false}
      gpuChartLoading={false}
      otherSensors={[]}
      metricFmt={(v, suffix = "") => (v == null ? "Indisponível" : `${v}${suffix}`)}
      formatByteRate={v => (v == null ? "—" : `${Math.round(v / 1024)} KB/s`)}
      formatCapacity={v => (v == null ? "—" : `${Math.round(v / 1024 ** 3)} GB`)}
      diskCapacityColor={() => "var(--neu-accent)"}
      getDiskCapacityHealth={() => "normal"}
      diskHealthTagClass={() => "healthy"}
    />
  );
}

describe("PerformanceModule", () => {
  it("makes the whole card clickable without redundant Expandir button and opens CPU details", () => {
    render(<Harness />);

    expect(screen.queryByText("Expandir")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Expandir detalhes de CPU" }));

    expect(screen.getByRole("button", { name: "Voltar para Desempenho" })).toBeInTheDocument();
    expect(screen.getAllByText("AMD Ryzen 7 7800X3D").length).toBeGreaterThan(0);
    expect(screen.getByLabelText("Uso por núcleo lógico")).toBeInTheDocument();

    // Abre seletor de métricas
    fireEvent.click(screen.getByRole("button", { name: /Escolher métricas/i }));
    expect(screen.getByLabelText("Seletor de métricas exibidas")).toBeInTheDocument();

    // Botão Voltar retorna à grade principal e suporta Enter no card da GPU
    fireEvent.click(screen.getByRole("button", { name: "Voltar para Desempenho" }));
    const gpuCard = screen.getByRole("button", { name: "Expandir detalhes de GPU" });
    fireEvent.keyDown(gpuCard, { key: "Enter" });
    expect(screen.getByText("Placa de Vídeo (GPU)")).toBeInTheDocument();
  });

  it("renders cards in preferences.cardOrder and reorders via move buttons and drag-and-drop without expanding", () => {
    const onPrefsChanged = vi.fn();
    const { container } = render(
      <Harness
        initialPrefs={{
          ...DEFAULT_TELEMETRY_PREFERENCES,
          cardOrder: ["gpu", "cpu", "ram", "network", "storage"],
        }}
        onPrefsChanged={onPrefsChanged}
      />,
    );

    const cardOrder = () =>
      Array.from(container.querySelectorAll(".focused-grid > section[data-group]")).map(el =>
        el.getAttribute("data-group"),
      );

    expect(cardOrder()).toEqual(["gpu", "cpu", "ram", "network", "storage"]);

    // Move CPU para trás sem expandir o card
    fireEvent.click(screen.getByRole("button", { name: "Mover CPU para trás" }));
    expect(screen.queryByRole("button", { name: "Voltar para Desempenho" })).not.toBeInTheDocument();
    expect(cardOrder()).toEqual(["cpu", "gpu", "ram", "network", "storage"]);
    expect(onPrefsChanged).toHaveBeenLastCalledWith(
      expect.objectContaining({
        cardOrder: ["cpu", "gpu", "ram", "network", "storage"],
      }),
    );

    // Drag-and-drop: arrasta "storage" até a posição de "cpu"
    const storageCard = container.querySelector('section[data-group="storage"]') as HTMLElement;
    const cpuCard = container.querySelector('section[data-group="cpu"]') as HTMLElement;
    const dataTransfer = {
      effectAllowed: "move",
      dropEffect: "move",
      setData: vi.fn(),
      getData: () => "storage",
    };

    fireEvent.dragStart(storageCard, { dataTransfer });
    fireEvent.dragOver(cpuCard, { dataTransfer });
    fireEvent.drop(cpuCard, { dataTransfer });
    fireEvent.dragEnd(storageCard);

    expect(cardOrder()).toEqual(["storage", "cpu", "gpu", "ram", "network"]);
    // Quando storage sobe para a primeira linha, não recebe span-2 para não quebrar a grade
    expect(storageCard).not.toHaveClass("span-2");
  });

  it("enforces the 8-metric limit for Tray pinning and allows unpinning + pinning", () => {
    render(
      <Harness
        initialPrefs={{
          ...DEFAULT_TELEMETRY_PREFERENCES,
          trayMetrics: ["cpu.usage", "ram.usage", "storage.system_disk"],
        }}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Sensores de hardware/i }));
    expect(screen.getByText("CPU Fan")).toBeInTheDocument();
    expect(screen.getByText(/1\.240 RPM/)).toBeInTheDocument();
    expect(screen.getByText(/Tray 3\/8/)).toBeInTheDocument();

    // Fixa o sensor de fan no 4º slot do Tray
    fireEvent.click(screen.getAllByTitle("Fixar no Tray")[0]);
    expect(screen.getByText(/Tray 4\/8/)).toBeInTheDocument();
  });

  it("shows explanatory fan diagnostic card when no fan sensors are detected", () => {
    const noFanSnapshot: HardwareTelemetrySnapshot = {
      ...sampleSnapshot,
      gpu: {
        ...sampleSnapshot.gpu,
        fanPercent: null,
        fanRpm: null,
      },
      metrics: [
        {
          id: "sensor.pdh-tz-tz00",
          group: "sensors",
          label: "Zona térmica TZ00",
          shortLabel: "ZONA TZ00",
          value: 43.2,
          unit: "°C",
          kind: "temperature",
          maxValue: 100,
          detail: "ACPI (PDH)",
          source: "ACPI (PDH)",
          unavailableReason: null,
        },
      ],
      externalProviders: [],
    };

    render(<Harness snapshot={noFanSnapshot} />);

    fireEvent.click(screen.getByRole("button", { name: /Sensores de hardware/i }));
    const diagnostic = screen.getByRole("status", { name: "Diagnóstico de sensores de ventoinha" });
    expect(diagnostic).toBeInTheDocument();
    expect(diagnostic).toHaveTextContent(/Nenhuma ventoinha detectada/i);
    expect(diagnostic).toHaveTextContent(/nvidia-smi/i);
    expect(diagnostic).toHaveTextContent(/HWiNFO Shared Memory/i);
    expect(diagnostic).toHaveTextContent(/LibreHardwareMonitor/i);
    expect(diagnostic).toHaveTextContent(/sem precisar reiniciar/i);

    // Continua visível ao filtrar especificamente por Ventoinhas
    fireEvent.click(screen.getByRole("button", { name: "Ventoinhas" }));
    expect(screen.getByRole("status", { name: "Diagnóstico de sensores de ventoinha" })).toBeInTheDocument();
  });

  it("sanitizes telemetry preferences capping tray metrics at 8 and completing cardOrder", () => {
    const clean = sanitizeTelemetryPreferences({
      trayMetrics: [
        "cpu.usage",
        "cpu.usage",
        "ram.usage",
        "gpu.temp",
        "gpu.fan_percent",
        "net.download",
        "net.upload",
        "disk.read_mbps",
        "disk.write_mbps",
        "cpu.temp",
      ],
      cardOrder: ["storage", "cpu", "storage"] as unknown as TelemetryPreferences["cardOrder"],
    });
    expect(clean.trayMetrics).toEqual([
      "cpu.usage",
      "ram.usage",
      "gpu.temp",
      "gpu.fan_percent",
      "net.download",
      "net.upload",
      "disk.read_mbps",
      "disk.write_mbps",
    ]);
    expect(clean.cardOrder).toEqual(["storage", "cpu", "ram", "gpu", "network"]);
  });
});
