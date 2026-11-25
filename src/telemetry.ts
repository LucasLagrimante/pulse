export type PerformanceGroup = "cpu" | "ram" | "gpu" | "network" | "storage" | "sensors";
export type PerformanceCardGroup = "cpu" | "ram" | "gpu" | "network" | "storage";

export type TelemetryMetric = {
  id: string;
  group: PerformanceGroup | string;
  label: string;
  shortLabel: string;
  value: number | null;
  unit: string;
  kind: string;
  maxValue?: number | null;
  detail?: string | null;
  source: string;
  unavailableReason?: string | null;
};

export type CpuHardwareInfo = {
  model?: string | null;
  physicalCores?: number | null;
  logicalCores: number;
  baseClockMhz?: number | null;
  currentClockMhz?: number | null;
  perCorePercent: number[];
  uptimeSeconds?: number | null;
  processCount?: number | null;
  threadCount?: number | null;
  handleCount?: number | null;
};

export type RamHardwareInfo = {
  usedGb?: number | null;
  totalGb?: number | null;
  availableGb?: number | null;
  cachedGb?: number | null;
  commitUsedGb?: number | null;
  commitLimitGb?: number | null;
  speedMhz?: number | null;
  slotsUsed?: number | null;
};

export type GpuHardwareInfo = {
  name?: string | null;
  usagePercent?: number | null;
  vramUsedGb?: number | null;
  vramTotalGb?: number | null;
  sharedUsedGb?: number | null;
  temperatureCelsius?: number | null;
  fanPercent?: number | null;
  fanRpm?: number | null;
  coreClockMhz?: number | null;
  memoryClockMhz?: number | null;
  powerWatts?: number | null;
  powerLimitWatts?: number | null;
  source?: string | null;
};

export type PhysicalDiskIo = {
  name: string;
  readMbps?: number | null;
  writeMbps?: number | null;
};

export type DiskIoStats = {
  readMbps?: number | null;
  writeMbps?: number | null;
  activePercent?: number | null;
  queueLength?: number | null;
  readIops?: number | null;
  writeIops?: number | null;
  perDisk?: PhysicalDiskIo[];
};

export type HardwareTelemetrySnapshot = {
  cpu: CpuHardwareInfo;
  ram: RamHardwareInfo;
  gpu: GpuHardwareInfo;
  diskIo: DiskIoStats;
  metrics: TelemetryMetric[];
  externalProviders: string[];
  sampledAtUnixMs: number;
};

export type TelemetryPreferences = {
  trayMetrics: string[];
  cardMetrics: Record<string, string[]>;
  cardOrder: PerformanceCardGroup[];
};

export const MAX_TRAY_METRICS = 8;

export const DEFAULT_CARD_ORDER: readonly PerformanceCardGroup[] = [
  "cpu",
  "ram",
  "gpu",
  "network",
  "storage",
] as const;

export const DEFAULT_TRAY_METRICS = [
  "cpu.usage",
  "ram.usage",
  "storage.system_disk",
  "gpu.usage",
] as const;

export const DEFAULT_TELEMETRY_PREFERENCES: TelemetryPreferences = {
  trayMetrics: [...DEFAULT_TRAY_METRICS],
  cardMetrics: {
    cpu: ["cpu.clock_ghz", "cpu.temp"],
    ram: ["ram.available_gb", "ram.cached_gb"],
    gpu: ["gpu.vram_used_gb", "gpu.temp", "gpu.fan_percent", "gpu.power_watts"],
    network: ["net.download", "net.upload"],
    storage: ["disk.read_mbps", "disk.write_mbps"],
  },
  cardOrder: [...DEFAULT_CARD_ORDER],
};

export const PERFORMANCE_GROUPS: { id: PerformanceGroup; label: string }[] = [
  { id: "cpu", label: "CPU" },
  { id: "ram", label: "Memória RAM" },
  { id: "gpu", label: "GPU" },
  { id: "network", label: "Rede" },
  { id: "storage", label: "Armazenamento" },
  { id: "sensors", label: "Sensores" },
];

export function isDefaultTraySelection(trayMetrics: string[]): boolean {
  if (trayMetrics.length !== DEFAULT_TRAY_METRICS.length) return false;
  return DEFAULT_TRAY_METRICS.every((id, index) => trayMetrics[index] === id);
}

export function sanitizeTelemetryPreferences(input?: Partial<TelemetryPreferences> | null): TelemetryPreferences {
  const rawTray = Array.isArray(input?.trayMetrics) ? input.trayMetrics : [...DEFAULT_TRAY_METRICS];
  const seenTray = new Set<string>();
  const trayMetrics: string[] = [];
  for (const item of rawTray) {
    if (typeof item !== "string") continue;
    const trimmed = item.trim();
    if (!trimmed || seenTray.has(trimmed)) continue;
    seenTray.add(trimmed);
    trayMetrics.push(trimmed);
    if (trayMetrics.length >= MAX_TRAY_METRICS) break;
  }

  const cardMetrics: Record<string, string[]> = { ...DEFAULT_TELEMETRY_PREFERENCES.cardMetrics };
  if (input?.cardMetrics && typeof input.cardMetrics === "object") {
    for (const [group, list] of Object.entries(input.cardMetrics)) {
      if (!Array.isArray(list)) continue;
      const seen = new Set<string>();
      cardMetrics[group] = list
        .filter((item): item is string => typeof item === "string" && item.trim().length > 0)
        .map(item => item.trim())
        .filter(item => {
          if (seen.has(item)) return false;
          seen.add(item);
          return true;
        })
        .slice(0, 12);
    }
  }

  const validGroups = new Set<string>(DEFAULT_CARD_ORDER);
  const rawOrder = Array.isArray(input?.cardOrder) ? input.cardOrder : [];
  const seenOrder = new Set<PerformanceCardGroup>();
  const cardOrder: PerformanceCardGroup[] = [];
  for (const item of rawOrder) {
    if (typeof item !== "string") continue;
    const normalized = item.trim().toLowerCase() as PerformanceCardGroup;
    if (validGroups.has(normalized) && !seenOrder.has(normalized)) {
      seenOrder.add(normalized);
      cardOrder.push(normalized);
    }
  }
  for (const group of DEFAULT_CARD_ORDER) {
    if (!seenOrder.has(group)) {
      seenOrder.add(group);
      cardOrder.push(group);
    }
  }

  return {
    trayMetrics: trayMetrics.length > 0 ? trayMetrics : [...DEFAULT_TRAY_METRICS],
    cardMetrics,
    cardOrder,
  };
}

export function telemetryMetricColor(metricItem: Pick<TelemetryMetric, "group" | "kind" | "id">): string {
  if (metricItem.id === "cpu.usage" || metricItem.group === "cpu") {
    return metricItem.kind === "temperature" ? "var(--neu-warning)" : "var(--neu-accent)";
  }
  if (metricItem.id === "ram.usage" || metricItem.group === "ram") {
    return "var(--neu-signal-secondary)";
  }
  if (metricItem.group === "gpu") {
    if (metricItem.kind === "temperature") return "var(--neu-warning)";
    if (metricItem.kind === "power") return "var(--neu-accent)";
    return "var(--neu-info)";
  }
  if (metricItem.group === "network") {
    return metricItem.id === "net.upload" ? "var(--neu-accent)" : "var(--neu-signal-secondary)";
  }
  if (metricItem.group === "storage") {
    return metricItem.id === "disk.write_mbps" ? "var(--neu-accent)" : "var(--neu-info)";
  }
  switch (metricItem.kind) {
    case "temperature":
      return "var(--neu-warning)";
    case "fan":
      return "var(--neu-info)";
    case "power":
      return "var(--neu-accent)";
    case "voltage":
      return "var(--neu-success)";
    default:
      return "var(--neu-signal-secondary)";
  }
}

export function formatTelemetryNumber(value: number | null | undefined, unit: string): string {
  if (value == null || typeof value !== "number" || !Number.isFinite(value)) {
    return "Indisponível";
  }
  if (unit === "B/s") {
    if (value >= 1024 ** 2) {
      return `${(value / 1024 ** 2).toLocaleString("pt-BR", { minimumFractionDigits: 1, maximumFractionDigits: 1 })} MB/s`;
    }
    if (value >= 1024) {
      return `${(value / 1024).toLocaleString("pt-BR", { minimumFractionDigits: 1, maximumFractionDigits: 1 })} KB/s`;
    }
    return `${Math.round(value).toLocaleString("pt-BR")} B/s`;
  }
  if (unit === "%") {
    return `${value.toLocaleString("pt-BR", { minimumFractionDigits: 0, maximumFractionDigits: 1 })}%`;
  }
  if (unit === "RPM" || unit === "") {
    return unit ? `${Math.round(value).toLocaleString("pt-BR")} ${unit}` : Math.round(value).toLocaleString("pt-BR");
  }
  if (unit === "GHz" || unit === "V") {
    return `${value.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 2 })} ${unit}`;
  }
  if (unit === "MHz") {
    return `${Math.round(value).toLocaleString("pt-BR")} MHz`;
  }
  const formatted = value.toLocaleString("pt-BR", {
    minimumFractionDigits: Math.abs(value) < 10 ? 1 : 0,
    maximumFractionDigits: 1,
  });
  return `${formatted} ${unit}`;
}

export function formatUptime(seconds?: number | null): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "Indisponível";
  const total = Math.floor(seconds);
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h ${minutes}m`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

export type UnifiedCatalogInput = {
  cpuPercent?: number | null;
  memoryPercent?: number | null;
  memoryUsedGb?: number | null;
  memoryTotalGb?: number | null;
  diskPercent?: number | null;
  diskUsedGb?: number | null;
  diskTotalGb?: number | null;
  systemDiskLabel?: string;
  gpuPercent?: number | null;
  gpuName?: string | null;
  downloadBps?: number | null;
  uploadBps?: number | null;
  cpuTempCelsius?: number | null;
  cpuTempSource?: string | null;
  gpuTempCelsius?: number | null;
  gpuTempSource?: string | null;
  extraThermalSensors?: Array<{ id: string; label: string; celsius: number; source: string }>;
  hardwareTelemetry?: HardwareTelemetrySnapshot | null;
};

/**
 * Monta o catálogo unificado de telemetria juntando overview, dispositivos e
 * telemetria expandida sem duplicar IDs.
 */
export function buildUnifiedMetrics(input: UnifiedCatalogInput): TelemetryMetric[] {
  const result: TelemetryMetric[] = [];
  const seen = new Set<string>();
  const add = (item: TelemetryMetric) => {
    if (seen.has(item.id)) return;
    seen.add(item.id);
    result.push(item);
  };

  add({
    id: "cpu.usage",
    group: "cpu",
    label: "Uso total da CPU",
    shortLabel: "CPU",
    value: typeof input.cpuPercent === "number" && Number.isFinite(input.cpuPercent) ? input.cpuPercent : null,
    unit: "%",
    kind: "percent",
    maxValue: 100,
    detail: input.hardwareTelemetry?.cpu?.model ?? null,
    source: "Sistema",
    unavailableReason: input.cpuPercent == null ? "Aguardando amostra da CPU" : null,
  });

  const ramDetail =
    typeof input.memoryUsedGb === "number" && typeof input.memoryTotalGb === "number"
      ? `${input.memoryUsedGb.toLocaleString("pt-BR", { maximumFractionDigits: 1 })} / ${input.memoryTotalGb.toLocaleString("pt-BR", { maximumFractionDigits: 1 })} GB`
      : null;
  add({
    id: "ram.usage",
    group: "ram",
    label: "Uso da Memória RAM",
    shortLabel: "RAM",
    value: typeof input.memoryPercent === "number" && Number.isFinite(input.memoryPercent) ? input.memoryPercent : null,
    unit: "%",
    kind: "percent",
    maxValue: 100,
    detail: ramDetail,
    source: "Sistema",
    unavailableReason: input.memoryPercent == null ? "Aguardando amostra de memória" : null,
  });

  const gpuVal =
    typeof input.gpuPercent === "number" && Number.isFinite(input.gpuPercent)
      ? input.gpuPercent
      : typeof input.hardwareTelemetry?.gpu?.usagePercent === "number"
        ? input.hardwareTelemetry.gpu.usagePercent
        : null;
  add({
    id: "gpu.usage",
    group: "gpu",
    label: "Uso da GPU",
    shortLabel: "GPU",
    value: gpuVal,
    unit: "%",
    kind: "percent",
    maxValue: 100,
    detail: input.gpuName ?? input.hardwareTelemetry?.gpu?.name ?? null,
    source: input.hardwareTelemetry?.gpu?.source ?? "PDH",
    unavailableReason: gpuVal == null ? "Utilização não exposta pelo driver" : null,
  });

  const diskDetail =
    typeof input.diskUsedGb === "number" && typeof input.diskTotalGb === "number"
      ? `${Math.round(input.diskUsedGb)} / ${Math.round(input.diskTotalGb)} GB`
      : null;
  add({
    id: "storage.system_disk",
    group: "storage",
    label: `Ocupação ${input.systemDiskLabel ?? "Disco C:"}`,
    shortLabel: (input.systemDiskLabel ?? "DISCO C:").toUpperCase(),
    value: typeof input.diskPercent === "number" && Number.isFinite(input.diskPercent) ? input.diskPercent : null,
    unit: "%",
    kind: "percent",
    maxValue: 100,
    detail: diskDetail,
    source: "Sistema",
    unavailableReason: input.diskPercent == null ? "Capacidade indisponível" : null,
  });

  add({
    id: "net.download",
    group: "network",
    label: "Download da rede",
    shortLabel: "REDE ↓",
    value:
      typeof input.downloadBps === "number" && Number.isFinite(input.downloadBps)
        ? input.downloadBps
        : null,
    unit: "B/s",
    kind: "rate",
    maxValue: null,
    detail: "Tráfego de entrada agregado",
    source: "Sistema",
    unavailableReason: input.downloadBps == null ? "Aguardando medição de rede" : null,
  });

  add({
    id: "net.upload",
    group: "network",
    label: "Upload da rede",
    shortLabel: "REDE ↑",
    value:
      typeof input.uploadBps === "number" && Number.isFinite(input.uploadBps)
        ? input.uploadBps
        : null,
    unit: "B/s",
    kind: "rate",
    maxValue: null,
    detail: "Tráfego de saída agregado",
    source: "Sistema",
    unavailableReason: input.uploadBps == null ? "Aguardando medição de rede" : null,
  });

  // Métricas vindas do backend `get_hardware_telemetry`
  if (input.hardwareTelemetry && Array.isArray(input.hardwareTelemetry.metrics)) {
    for (const item of input.hardwareTelemetry.metrics) {
      add(item);
    }
  }

  // Fallback de temperatura CPU/GPU vindo de `get_performance_details` (quando ainda não entrou pelo snapshot)
  if (typeof input.cpuTempCelsius === "number" && Number.isFinite(input.cpuTempCelsius)) {
    add({
      id: "cpu.temp",
      group: "cpu",
      label: "Temperatura da CPU",
      shortLabel: "TEMP CPU",
      value: input.cpuTempCelsius,
      unit: "°C",
      kind: "temperature",
      maxValue: 100,
      detail: input.cpuTempSource ?? null,
      source: input.cpuTempSource ?? "Sensor",
      unavailableReason: null,
    });
  }

  if (typeof input.gpuTempCelsius === "number" && Number.isFinite(input.gpuTempCelsius)) {
    add({
      id: "gpu.temp",
      group: "gpu",
      label: "Temperatura da GPU",
      shortLabel: "TEMP GPU",
      value: input.gpuTempCelsius,
      unit: "°C",
      kind: "temperature",
      maxValue: 100,
      detail: input.gpuName ?? input.gpuTempSource ?? null,
      source: input.gpuTempSource ?? "Sensor",
      unavailableReason: null,
    });
  }

  if (Array.isArray(input.extraThermalSensors)) {
    for (const sensor of input.extraThermalSensors) {
      add({
        id: `sensor.${sensor.id}`,
        group: "sensors",
        label: sensor.label,
        shortLabel: sensor.label.slice(0, 14).toUpperCase(),
        value: sensor.celsius,
        unit: "°C",
        kind: "temperature",
        maxValue: 100,
        detail: sensor.source,
        source: sensor.source,
        unavailableReason: null,
      });
    }
  }

  return result;
}
