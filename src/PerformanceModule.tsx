import { useMemo, useState, type DragEvent, type KeyboardEvent as ReactKeyboardEvent } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronRight,
  Cpu,
  Fan,
  Gauge,
  GripVertical,
  HardDrive,
  MemoryStick,
  Network,
  Pin,
  PinOff,
  SlidersHorizontal,
  Thermometer,
} from "lucide-react";
import { DetailedResourceChart, HardwareGaugeBar, MiniCapacityBar, SparklineChart } from "./Charts";
import {
  DEFAULT_CARD_ORDER,
  MAX_TRAY_METRICS,
  PERFORMANCE_GROUPS,
  formatTelemetryNumber,
  formatUptime,
  telemetryMetricColor,
  type HardwareTelemetrySnapshot,
  type PerformanceCardGroup,
  type PerformanceGroup,
  type TelemetryMetric,
  type TelemetryPreferences,
} from "./telemetry";
import "./PerformanceModule.css";

type CapabilityLike<T> = {
  value?: T | null;
  unavailableReason?: string | null;
};

type ThermalSensorLike = {
  id: string;
  label: string;
  celsius: number;
  source: string;
};

type NetworkAdapterLike = {
  name: string;
  description?: string;
  status: string;
  hardwareInterface?: boolean;
  linkSpeedBps?: CapabilityLike<number>;
  receivedBps?: CapabilityLike<number>;
  sentBps?: CapabilityLike<number>;
};

type PhysicalDiskLike = {
  index: number;
  model: string;
  mediaType: string;
  busType: string;
  sizeBytes?: CapabilityLike<number>;
  health?: CapabilityLike<string>;
  temperatureCelsius?: CapabilityLike<number>;
  wearPercent?: number | null;
  powerOnHours?: number | null;
};

type VolumeLike = {
  path: string;
  label?: string | null;
  totalBytes?: number | null;
  freeBytes?: number | null;
};

export type PerformanceModuleProps = {
  overview: {
    cpuPercent?: number | null;
    gpuPercent?: number | null;
    memoryUsedGb?: number | null;
    memoryTotalGb?: number | null;
    diskUsedGb?: number | null;
    diskTotalGb?: number | null;
  } | null;
  performance: {
    gpu?: CapabilityLike<string>;
    vramBytes?: CapabilityLike<number>;
    thermalSensors?: ThermalSensorLike[];
  } | null;
  devices: {
    adapters?: CapabilityLike<NetworkAdapterLike[]>;
    disks?: CapabilityLike<PhysicalDiskLike[]>;
    volumes?: VolumeLike[];
  } | null;
  hardwareTelemetry: HardwareTelemetrySnapshot | null;
  unifiedMetrics: TelemetryMetric[];
  metricHistories: Record<string, number[]>;
  preferences: TelemetryPreferences;
  onUpdatePreferences: (next: TelemetryPreferences) => void;
  selectedGroup: PerformanceGroup | null;
  onSelectGroup: (group: PerformanceGroup | null) => void;
  cpuHistory: number[];
  ramHistory: number[];
  gpuHistory: number[];
  downloadHistory: number[];
  uploadHistory: number[];
  cpuChartLoading: boolean;
  ramChartLoading: boolean;
  gpuChartLoading: boolean;
  cpuSensor?: ThermalSensorLike;
  gpuSensor?: ThermalSensorLike;
  otherSensors: ThermalSensorLike[];
  metricFmt: (value: number | null | undefined, suffix?: string) => string;
  formatByteRate: (value?: number | null) => string;
  formatCapacity: (value?: number | null) => string;
  diskCapacityColor: (health: "normal" | "warning" | "critical" | "unknown") => string;
  getDiskCapacityHealth: (percent: number | null | undefined) => "normal" | "warning" | "critical" | "unknown";
  diskHealthTagClass: (health?: string | null) => string;
};

const GROUP_META: Record<PerformanceGroup, { title: string; subtitle: string }> = {
  cpu: { title: "Processador (CPU)", subtitle: "Carga por núcleo, frequência, threads e sensores" },
  ram: { title: "Memória RAM", subtitle: "Alocação física, cache standby e memória comprometida" },
  gpu: { title: "Placa de Vídeo (GPU)", subtitle: "Motor gráfico, VRAM global, temperatura, fan e energia" },
  network: { title: "Rede", subtitle: "Throughput em tempo real e adaptadores físicos/virtuais" },
  storage: { title: "Armazenamento Físico", subtitle: "Leitura/escrita, IOPS, volumes e saúde dos discos" },
  sensors: { title: "Sensores de Hardware", subtitle: "Temperaturas, ventoinhas (RPM), potência e tensões" },
};

const CARD_ACCESSIBLE_NAME: Record<PerformanceCardGroup, string> = {
  cpu: "CPU",
  ram: "Memória RAM",
  gpu: "GPU",
  network: "Rede",
  storage: "discos físicos",
};

function isFanMetric(metricItem: TelemetryMetric): boolean {
  return (
    metricItem.kind === "fan" ||
    metricItem.id === "gpu.fan_percent" ||
    metricItem.id === "gpu.fan_rpm"
  );
}

function GroupIcon({ group }: { group: PerformanceGroup }) {
  switch (group) {
    case "cpu":
      return <Cpu size={15} aria-hidden="true" />;
    case "ram":
      return <MemoryStick size={15} aria-hidden="true" />;
    case "gpu":
      return <Gauge size={15} aria-hidden="true" />;
    case "network":
      return <Network size={15} aria-hidden="true" />;
    case "storage":
      return <HardDrive size={15} aria-hidden="true" />;
    case "sensors":
      return <Thermometer size={15} aria-hidden="true" />;
  }
}

function formatLinkSpeed(bps?: number | null): string {
  if (bps == null || !Number.isFinite(bps) || bps <= 0) return "Link indisponível";
  if (bps >= 1_000_000_000) return `${(bps / 1_000_000_000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} Gbps`;
  if (bps >= 1_000_000) return `${Math.round(bps / 1_000_000).toLocaleString("pt-BR")} Mbps`;
  return `${Math.round(bps / 1_000).toLocaleString("pt-BR")} Kbps`;
}

export function PerformanceModule({
  overview,
  performance,
  devices,
  hardwareTelemetry,
  unifiedMetrics,
  metricHistories,
  preferences,
  onUpdatePreferences,
  selectedGroup,
  onSelectGroup,
  cpuHistory,
  ramHistory,
  gpuHistory,
  downloadHistory,
  uploadHistory,
  cpuChartLoading,
  ramChartLoading,
  gpuChartLoading,
  cpuSensor,
  gpuSensor,
  otherSensors,
  metricFmt,
  formatByteRate,
  formatCapacity,
  diskCapacityColor,
  getDiskCapacityHealth,
  diskHealthTagClass,
}: PerformanceModuleProps) {
  const [sensorFilter, setSensorFilter] = useState<string>("all");
  const [pickerOpen, setPickerOpen] = useState<boolean>(false);
  const [draggedGroup, setDraggedGroup] = useState<PerformanceCardGroup | null>(null);
  const [dragOverGroup, setDragOverGroup] = useState<PerformanceCardGroup | null>(null);

  const memoryPercent =
    overview &&
    typeof overview.memoryTotalGb === "number" &&
    overview.memoryTotalGb > 0 &&
    typeof overview.memoryUsedGb === "number"
      ? Math.round((overview.memoryUsedGb / overview.memoryTotalGb) * 100)
      : undefined;

  const gpuPercent =
    overview && typeof overview.gpuPercent === "number" && Number.isFinite(overview.gpuPercent)
      ? Math.max(0, Math.min(100, overview.gpuPercent))
      : typeof hardwareTelemetry?.gpu?.usagePercent === "number"
        ? Math.max(0, Math.min(100, hardwareTelemetry.gpu.usagePercent))
        : undefined;

  const showGpuChart =
    gpuPercent !== undefined || gpuHistory.some(value => Number.isFinite(value));

  const vramTotalGb = hardwareTelemetry?.gpu?.vramTotalGb ?? null;
  const vramUsedGb = hardwareTelemetry?.gpu?.vramUsedGb ?? null;
  const legacyVramBytes =
    typeof performance?.vramBytes?.value === "number" && performance.vramBytes.value > 0
      ? performance.vramBytes.value
      : undefined;

  const networkAdapters = Array.isArray(devices?.adapters?.value) ? devices.adapters.value : [];
  const activeAdapterCount = networkAdapters.filter(a => a.status === "Ativo").length;
  const physicalDisks = Array.isArray(devices?.disks?.value) ? devices.disks.value : [];
  const volumes = useMemo(
    () => (Array.isArray(devices?.volumes) ? devices.volumes : []),
    [devices?.volumes],
  );

  const capacity = useMemo(() => {
    let total = 0;
    let free = 0;
    let measured = false;
    for (const vol of volumes) {
      if (
        typeof vol.totalBytes === "number" &&
        typeof vol.freeBytes === "number" &&
        Number.isFinite(vol.totalBytes) &&
        Number.isFinite(vol.freeBytes) &&
        vol.totalBytes > 0 &&
        vol.freeBytes >= 0 &&
        vol.freeBytes <= vol.totalBytes
      ) {
        total += vol.totalBytes;
        free += vol.freeBytes;
        measured = true;
      }
    }
    return { total, free, measured };
  }, [volumes]);

  const orderedCardGroups = useMemo<PerformanceCardGroup[]>(() => {
    const valid = new Set<string>(DEFAULT_CARD_ORDER);
    const seen = new Set<PerformanceCardGroup>();
    const list: PerformanceCardGroup[] = [];
    const raw = Array.isArray(preferences.cardOrder) ? preferences.cardOrder : [];
    for (const item of raw) {
      if (valid.has(item) && !seen.has(item)) {
        seen.add(item);
        list.push(item);
      }
    }
    for (const fallback of DEFAULT_CARD_ORDER) {
      if (!seen.has(fallback)) {
        seen.add(fallback);
        list.push(fallback);
      }
    }
    return list;
  }, [preferences.cardOrder]);

  const metricsById = useMemo(() => {
    const map = new Map<string, TelemetryMetric>();
    for (const item of unifiedMetrics) {
      map.set(item.id, item);
    }
    return map;
  }, [unifiedMetrics]);

  const metricsForGroup = (group: PerformanceGroup): TelemetryMetric[] => {
    if (group === "sensors") {
      return unifiedMetrics.filter(
        m =>
          m.group === "sensors" ||
          m.kind === "temperature" ||
          isFanMetric(m) ||
          m.kind === "power" ||
          m.kind === "voltage" ||
          m.id.startsWith("sensor."),
      );
    }
    return unifiedMetrics.filter(m => m.group === group);
  };

  const hasFanSensors = useMemo(() => {
    const anyMeasuredFanMetric = unifiedMetrics.some(m => m.value != null && isFanMetric(m));
    return (
      anyMeasuredFanMetric ||
      hardwareTelemetry?.gpu?.fanPercent != null ||
      hardwareTelemetry?.gpu?.fanRpm != null
    );
  }, [unifiedMetrics, hardwareTelemetry?.gpu?.fanPercent, hardwareTelemetry?.gpu?.fanRpm]);

  const selectedCardMetrics = (group: PerformanceGroup): TelemetryMetric[] => {
    const configured = preferences.cardMetrics[group] ?? [];
    return configured
      .map(id => metricsById.get(id))
      .filter((m): m is TelemetryMetric => Boolean(m && m.value != null));
  };

  const toggleCardMetric = (group: PerformanceGroup, metricId: string) => {
    const current = preferences.cardMetrics[group] ?? [];
    const exists = current.includes(metricId);
    const nextGroup = exists
      ? current.filter(id => id !== metricId)
      : [...current, metricId].slice(0, 10);
    onUpdatePreferences({
      ...preferences,
      cardMetrics: {
        ...preferences.cardMetrics,
        [group]: nextGroup,
      },
    });
  };

  const toggleTrayMetric = (metricId: string) => {
    const current = preferences.trayMetrics;
    if (current.includes(metricId)) {
      if (current.length <= 1) return;
      onUpdatePreferences({
        ...preferences,
        trayMetrics: current.filter(id => id !== metricId),
      });
      return;
    }
    if (current.length >= MAX_TRAY_METRICS) return;
    onUpdatePreferences({
      ...preferences,
      trayMetrics: [...current, metricId],
    });
  };

  const moveCardByOffset = (group: PerformanceCardGroup, offset: -1 | 1) => {
    const idx = orderedCardGroups.indexOf(group);
    const targetIdx = idx + offset;
    if (idx < 0 || targetIdx < 0 || targetIdx >= orderedCardGroups.length) return;
    const nextOrder = [...orderedCardGroups];
    [nextOrder[idx], nextOrder[targetIdx]] = [nextOrder[targetIdx], nextOrder[idx]];
    onUpdatePreferences({
      ...preferences,
      cardOrder: nextOrder,
    });
  };

  const moveCardToTarget = (source: PerformanceCardGroup, target: PerformanceCardGroup) => {
    if (source === target) return;
    const fromIdx = orderedCardGroups.indexOf(source);
    const toIdx = orderedCardGroups.indexOf(target);
    if (fromIdx < 0 || toIdx < 0) return;
    const nextOrder = [...orderedCardGroups];
    const [moved] = nextOrder.splice(fromIdx, 1);
    nextOrder.splice(toIdx, 0, moved);
    onUpdatePreferences({
      ...preferences,
      cardOrder: nextOrder,
    });
  };

  const handleCardDragStart = (group: PerformanceCardGroup) => (event: DragEvent<HTMLElement>) => {
    setDraggedGroup(group);
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", group);
  };

  const handleCardDragOver = (group: PerformanceCardGroup) => (event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    if (dragOverGroup !== group) {
      setDragOverGroup(group);
    }
  };

  const handleCardDragLeave = (group: PerformanceCardGroup) => (event: DragEvent<HTMLElement>) => {
    const related = event.relatedTarget as Node | null;
    if (related && event.currentTarget.contains(related)) return;
    if (dragOverGroup === group) {
      setDragOverGroup(null);
    }
  };

  const handleCardDrop = (target: PerformanceCardGroup) => (event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    const fromTransfer = event.dataTransfer.getData("text/plain") as PerformanceCardGroup;
    const source =
      draggedGroup ?? (DEFAULT_CARD_ORDER.includes(fromTransfer) ? fromTransfer : null);
    setDraggedGroup(null);
    setDragOverGroup(null);
    if (source && source !== target) {
      moveCardToTarget(source, target);
    }
  };

  const handleCardDragEnd = () => {
    setDraggedGroup(null);
    setDragOverGroup(null);
  };

  const getHistoryForMetric = (metricItem: TelemetryMetric): number[] => {
    if (metricItem.id === "cpu.usage") return cpuHistory;
    if (metricItem.id === "ram.usage") return ramHistory;
    if (metricItem.id === "gpu.usage") return gpuHistory;
    if (metricItem.id === "net.download") return downloadHistory;
    if (metricItem.id === "net.upload") return uploadHistory;
    return metricHistories[metricItem.id] ?? (metricItem.value != null ? [metricItem.value] : []);
  };

  const renderCardSubMetrics = (group: PerformanceGroup) => {
    const items = selectedCardMetrics(group).slice(0, 4);
    if (items.length === 0) return null;
    return (
      <div className="perf-card-pills" aria-label={`Métricas selecionadas de ${GROUP_META[group].title}`}>
        {items.map(item => (
          <span key={item.id} className="perf-metric-pill" title={`${item.label} · ${item.source}`}>
            <span className="perf-metric-pill-label">{item.shortLabel}</span>
            <strong>{formatTelemetryNumber(item.value, item.unit)}</strong>
          </span>
        ))}
      </div>
    );
  };

  const openGroup = (group: PerformanceGroup) => {
    setPickerOpen(false);
    setSensorFilter("all");
    onSelectGroup(group);
  };

  const handleCardKeyDown = (group: PerformanceCardGroup) => (event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.target !== event.currentTarget) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openGroup(group);
    }
  };

  // ==========================================================================
  // MODO EXPANDIDO (com botão Voltar + troca rápida + seletor de métricas)
  // ==========================================================================
  if (selectedGroup !== null) {
    const groupMetrics = metricsForGroup(selectedGroup);
    const activeIds = new Set(preferences.cardMetrics[selectedGroup] ?? []);
    const primaryIdsForGroup: Record<PerformanceGroup, string[]> = {
      cpu: ["cpu.usage"],
      ram: ["ram.usage"],
      gpu: ["gpu.usage"],
      network: ["net.download", "net.upload"],
      storage: ["disk.read_mbps", "disk.write_mbps"],
      sensors: [],
    };
    const chartMetrics = groupMetrics.filter(
      m =>
        (primaryIdsForGroup[selectedGroup].includes(m.id) || activeIds.has(m.id)) &&
        m.value != null,
    );
    const filteredSensorList =
      selectedGroup === "sensors"
        ? groupMetrics.filter(m => {
            if (sensorFilter === "all") return true;
            if (sensorFilter === "fan") return isFanMetric(m);
            return m.kind === sensorFilter;
          })
        : groupMetrics;

    const trayFull = preferences.trayMetrics.length >= MAX_TRAY_METRICS;
    const showFanDiagnosticCard =
      selectedGroup === "sensors" &&
      !hasFanSensors &&
      (sensorFilter === "all" || sensorFilter === "fan");

    return (
      <section className="module-dashboard perf-expanded-view" aria-label={`Detalhes de ${GROUP_META[selectedGroup].title}`}>
        <header className="panel perf-expanded-header">
          <div className="perf-expanded-header-left">
            <button
              type="button"
              className="perf-back-btn"
              onClick={() => onSelectGroup(null)}
              aria-label="Voltar para Desempenho"
            >
              <ArrowLeft size={16} aria-hidden="true" />
              <span>Voltar</span>
            </button>
            <div className="perf-expanded-title-group">
              <div className="perf-expanded-kicker">
                <GroupIcon group={selectedGroup} />
                <span>DESEMPENHO DETALHADO</span>
              </div>
              <h3>{GROUP_META[selectedGroup].title}</h3>
              <p>{GROUP_META[selectedGroup].subtitle}</p>
            </div>
          </div>

          <div className="perf-expanded-header-actions">
            <span className="perf-tray-counter" title="Slots de gráficos configurados no Painel Rápido (Tray)">
              <Pin size={13} aria-hidden="true" />
              Tray {preferences.trayMetrics.length}/{MAX_TRAY_METRICS}
            </span>
            <button
              type="button"
              className={`perf-picker-toggle ${pickerOpen ? "is-active" : ""}`}
              onClick={() => setPickerOpen(v => !v)}
              aria-expanded={pickerOpen}
            >
              <SlidersHorizontal size={14} aria-hidden="true" />
              <span>Escolher métricas ({groupMetrics.length})</span>
            </button>
          </div>
        </header>

        <nav className="perf-group-tabs" aria-label="Subsistemas de desempenho">
          {PERFORMANCE_GROUPS.map(g => (
            <button
              key={g.id}
              type="button"
              className={`perf-group-tab ${selectedGroup === g.id ? "is-active" : ""}`}
              onClick={() => openGroup(g.id)}
            >
              <GroupIcon group={g.id} />
              <span>{g.label}</span>
            </button>
          ))}
        </nav>

        {pickerOpen && (
          <section className="panel perf-metric-picker" aria-label="Seletor de métricas exibidas">
            <div className="perf-picker-head">
              <div>
                <strong>Métricas disponíveis neste subsistema</strong>
                <small>
                  Marque para exibir no card e nos gráficos ao vivo, ou fixe até {MAX_TRAY_METRICS} no Painel Rápido do tray.
                </small>
              </div>
              {trayFull && (
                <span className="perf-picker-limit-note">
                  Tray cheio ({MAX_TRAY_METRICS}/{MAX_TRAY_METRICS}) — desafixe uma métrica para trocar
                </span>
              )}
            </div>
            <div className="perf-picker-grid">
              {groupMetrics.map(item => {
                const isPrimary = primaryIdsForGroup[selectedGroup].includes(item.id);
                const isShown = isPrimary || activeIds.has(item.id);
                const isPinnedTray = preferences.trayMetrics.includes(item.id);
                const canPinTray = isPinnedTray || !trayFull;
                return (
                  <div key={item.id} className={`perf-picker-item ${isShown ? "is-selected" : ""}`}>
                    <div className="perf-picker-item-info">
                      <span className="perf-picker-item-label">{item.label}</span>
                      <span className="perf-picker-item-meta">
                        {formatTelemetryNumber(item.value, item.unit)} · {item.source}
                      </span>
                    </div>
                    <div className="perf-picker-item-actions">
                      <button
                        type="button"
                        className={`perf-toggle-chip ${isShown ? "is-on" : ""}`}
                        disabled={isPrimary}
                        onClick={() => toggleCardMetric(selectedGroup, item.id)}
                        title={isPrimary ? "Métrica principal deste painel" : isShown ? "Ocultar métrica" : "Exibir métrica"}
                      >
                        <Check size={12} aria-hidden="true" />
                        <span>{isPrimary ? "Principal" : isShown ? "Visível" : "Exibir"}</span>
                      </button>
                      <button
                        type="button"
                        className={`perf-pin-chip ${isPinnedTray ? "is-pinned" : ""}`}
                        disabled={!canPinTray && !isPinnedTray}
                        onClick={() => toggleTrayMetric(item.id)}
                        title={
                          isPinnedTray
                            ? "Remover do Painel Rápido do tray"
                            : trayFull
                              ? `Limite de ${MAX_TRAY_METRICS} gráficos no tray atingido`
                              : "Fixar no Painel Rápido do tray"
                        }
                      >
                        {isPinnedTray ? <PinOff size={12} aria-hidden="true" /> : <Pin size={12} aria-hidden="true" />}
                        <span>{isPinnedTray ? "No Tray" : "Tray"}</span>
                      </button>
                    </div>
                  </div>
                );
              })}
              {groupMetrics.length === 0 && (
                <p className="perf-empty-hint">
                  Nenhuma métrica adicional foi reportada pelo hardware ou driver para este subsistema.
                </p>
              )}
            </div>
          </section>
        )}

        {/* Faixa de especificações de hardware do subsistema */}
        {selectedGroup === "cpu" && (
          <section className="panel perf-specs-strip" aria-label="Especificações do processador">
            <div className="perf-spec-cell">
              <span>PROCESSADOR</span>
              <strong>{hardwareTelemetry?.cpu?.model || "Não reportado"}</strong>
            </div>
            <div className="perf-spec-cell">
              <span>NÚCLEOS</span>
              <strong>
                {hardwareTelemetry?.cpu?.physicalCores
                  ? `${hardwareTelemetry.cpu.physicalCores} físicos · ${hardwareTelemetry.cpu.logicalCores} lógicos`
                  : `${hardwareTelemetry?.cpu?.logicalCores ?? "—"} lógicos`}
              </strong>
            </div>
            <div className="perf-spec-cell">
              <span>FREQUÊNCIA</span>
              <strong>
                {hardwareTelemetry?.cpu?.currentClockMhz
                  ? `${(hardwareTelemetry.cpu.currentClockMhz / 1000).toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 2 })} GHz`
                  : "Indisponível"}
              </strong>
            </div>
            <div className="perf-spec-cell">
              <span>ATIVIDADE</span>
              <strong>{formatUptime(hardwareTelemetry?.cpu?.uptimeSeconds)}</strong>
            </div>
            <div className="perf-spec-cell">
              <span>PROCESSOS / THREADS</span>
              <strong>
                {hardwareTelemetry?.cpu?.processCount ?? "—"} / {hardwareTelemetry?.cpu?.threadCount ?? "—"}
              </strong>
            </div>
          </section>
        )}

        {selectedGroup === "ram" && (
          <section className="panel perf-specs-strip" aria-label="Especificações da memória RAM">
            <div className="perf-spec-cell">
              <span>EM USO / TOTAL</span>
              <strong>
                {metricFmt(overview?.memoryUsedGb, " GB")} / {metricFmt(overview?.memoryTotalGb, " GB")}
              </strong>
            </div>
            <div className="perf-spec-cell">
              <span>DISPONÍVEL</span>
              <strong>{formatTelemetryNumber(hardwareTelemetry?.ram?.availableGb, "GB")}</strong>
            </div>
            <div className="perf-spec-cell">
              <span>EM CACHE</span>
              <strong>{formatTelemetryNumber(hardwareTelemetry?.ram?.cachedGb, "GB")}</strong>
            </div>
            <div className="perf-spec-cell">
              <span>COMPROMETIDA</span>
              <strong>
                {hardwareTelemetry?.ram?.commitUsedGb != null
                  ? `${formatTelemetryNumber(hardwareTelemetry.ram.commitUsedGb, "GB")} / ${formatTelemetryNumber(hardwareTelemetry.ram.commitLimitGb, "GB")}`
                  : "Indisponível"}
              </strong>
            </div>
            <div className="perf-spec-cell">
              <span>VELOCIDADE / MÓDULOS</span>
              <strong>
                {hardwareTelemetry?.ram?.speedMhz
                  ? `${hardwareTelemetry.ram.speedMhz} MHz${hardwareTelemetry.ram.slotsUsed ? ` · ${hardwareTelemetry.ram.slotsUsed} módulos` : ""}`
                  : "Não exposto"}
              </strong>
            </div>
          </section>
        )}

        {selectedGroup === "gpu" && (
          <section className="panel perf-specs-strip" aria-label="Especificações da GPU">
            <div className="perf-spec-cell">
              <span>CONTROLADORA</span>
              <strong>{hardwareTelemetry?.gpu?.name || performance?.gpu?.value || "Indisponível"}</strong>
            </div>
            <div className="perf-spec-cell">
              <span>VRAM DEDICADA</span>
              <strong>
                {vramUsedGb != null && vramTotalGb != null
                  ? `${formatTelemetryNumber(vramUsedGb, "GB")} / ${formatTelemetryNumber(vramTotalGb, "GB")}`
                  : vramTotalGb != null
                    ? formatTelemetryNumber(vramTotalGb, "GB")
                    : "Indisponível"}
              </strong>
            </div>
            <div className="perf-spec-cell">
              <span>TEMPERATURA / FAN</span>
              <strong>
                {hardwareTelemetry?.gpu?.temperatureCelsius != null
                  ? `${metricFmt(hardwareTelemetry.gpu.temperatureCelsius)} °C`
                  : gpuSensor
                    ? `${metricFmt(gpuSensor.celsius)} °C`
                    : "—"}
                {hardwareTelemetry?.gpu?.fanPercent != null
                  ? ` · ${Math.round(hardwareTelemetry.gpu.fanPercent)}%`
                  : ""}
              </strong>
            </div>
            <div className="perf-spec-cell">
              <span>CLOCKS (CORE / VRAM)</span>
              <strong>
                {hardwareTelemetry?.gpu?.coreClockMhz != null
                  ? `${Math.round(hardwareTelemetry.gpu.coreClockMhz)} MHz`
                  : "—"}
                {hardwareTelemetry?.gpu?.memoryClockMhz != null
                  ? ` / ${Math.round(hardwareTelemetry.gpu.memoryClockMhz)} MHz`
                  : ""}
              </strong>
            </div>
            <div className="perf-spec-cell">
              <span>ENERGIA</span>
              <strong>
                {hardwareTelemetry?.gpu?.powerWatts != null
                  ? `${formatTelemetryNumber(hardwareTelemetry.gpu.powerWatts, "W")}${hardwareTelemetry.gpu.powerLimitWatts ? ` / ${Math.round(hardwareTelemetry.gpu.powerLimitWatts)} W` : ""}`
                  : "Não exposto"}
              </strong>
            </div>
          </section>
        )}

        {/* Grade de gráficos em tempo real das métricas selecionadas */}
        {selectedGroup !== "sensors" && chartMetrics.length > 0 && (
          <div className="perf-expanded-charts-grid">
            {chartMetrics.map(item => {
              const history = getHistoryForMetric(item);
              const isAutoScale = item.maxValue == null && item.unit !== "%";
              const maxVal = item.maxValue ?? (item.unit === "%" ? 100 : 10);
              return (
                <section key={item.id} className="panel perf-expanded-chart-card">
                  <div className="perf-expanded-chart-top">
                    <span className="perf-expanded-chart-source">{item.source}</span>
                    <button
                      type="button"
                      className={`perf-pin-chip ${preferences.trayMetrics.includes(item.id) ? "is-pinned" : ""}`}
                      disabled={!preferences.trayMetrics.includes(item.id) && trayFull}
                      onClick={() => toggleTrayMetric(item.id)}
                      title={
                        preferences.trayMetrics.includes(item.id)
                          ? "Remover do Painel Rápido do tray"
                          : trayFull
                            ? `Limite de ${MAX_TRAY_METRICS} gráficos no tray atingido`
                            : "Fixar no Painel Rápido do tray"
                      }
                    >
                      {preferences.trayMetrics.includes(item.id) ? <PinOff size={12} /> : <Pin size={12} />}
                      <span>{preferences.trayMetrics.includes(item.id) ? "No Tray" : "Fixar no Tray"}</span>
                    </button>
                  </div>
                  <DetailedResourceChart
                    title={item.label.toUpperCase()}
                    history={history}
                    currentValue={formatTelemetryNumber(item.value, item.unit)}
                    subValue={item.detail ?? undefined}
                    color={telemetryMetricColor(item)}
                    unit={item.unit}
                    max={maxVal}
                    autoScale={isAutoScale}
                    formatValue={v => formatTelemetryNumber(v, item.unit)}
                    ariaLabel={`Gráfico detalhado de ${item.label}`}
                  />
                </section>
              );
            })}
          </div>
        )}

        {/* Detalhes específicos por subsistema */}
        {selectedGroup === "cpu" &&
          hardwareTelemetry &&
          hardwareTelemetry.cpu.perCorePercent.length > 0 && (
            <section className="panel perf-cores-panel" aria-label="Uso por núcleo lógico">
              <div className="perf-section-head">
                <h4>Uso por núcleo lógico ({hardwareTelemetry.cpu.perCorePercent.length})</h4>
                <small>Leitura individual em tempo real</small>
              </div>
              <div className="perf-cores-grid">
                {hardwareTelemetry.cpu.perCorePercent.map((pct, index) => (
                  <div key={index} className="perf-core-cell">
                    <div className="perf-core-head">
                      <span>C{index}</span>
                      <strong>{Math.round(pct)}%</strong>
                    </div>
                    <div className="mini-capacity-bar-track">
                      <div
                        className="mini-capacity-bar-fill"
                        style={{
                          width: `${Math.max(0, Math.min(100, pct))}%`,
                          backgroundColor:
                            pct >= 85
                              ? "var(--neu-danger)"
                              : pct >= 65
                                ? "var(--neu-warning)"
                                : "var(--neu-accent)",
                        }}
                      />
                    </div>
                  </div>
                ))}
              </div>
            </section>
          )}

        {selectedGroup === "network" && (
          <section className="panel perf-detail-table-panel" aria-label="Adaptadores de rede detalhados">
            <div className="perf-section-head">
              <h4>Adaptadores de rede ({networkAdapters.length})</h4>
              <small>{activeAdapterCount} ativo(s)</small>
            </div>
            <ul className="perf-detail-list">
              {networkAdapters.map((adapter, index) => (
                <li key={`${adapter.name}-${index}`} className="perf-detail-row">
                  <div className="perf-detail-main">
                    <span className={`network-dot ${adapter.status === "Ativo" ? "is-active" : ""}`} aria-hidden="true" />
                    <div>
                      <strong>{adapter.name}</strong>
                      <small>{adapter.description || adapter.status}</small>
                    </div>
                  </div>
                  <div className="perf-detail-values">
                    <span className="storage-tag">{formatLinkSpeed(adapter.linkSpeedBps?.value)}</span>
                    <span className="perf-detail-rate">
                      ↓ {formatByteRate(adapter.receivedBps?.value)} · ↑ {formatByteRate(adapter.sentBps?.value)}
                    </span>
                  </div>
                </li>
              ))}
              {networkAdapters.length === 0 && (
                <li className="storage-empty">
                  {devices?.adapters?.unavailableReason || "Nenhum adaptador de rede reportado"}
                </li>
              )}
            </ul>
          </section>
        )}

        {selectedGroup === "storage" && (
          <section className="panel perf-detail-table-panel" aria-label="Volumes e discos físicos detalhados">
            <div className="storage-device-body">
              <div className="storage-column">
                <span className="storage-column-label">VOLUMES ({volumes.length})</span>
                <ul className="storage-volume-list">
                  {volumes.map(volume => {
                    const hasCapacity =
                      typeof volume.totalBytes === "number" &&
                      typeof volume.freeBytes === "number" &&
                      Number.isFinite(volume.totalBytes) &&
                      Number.isFinite(volume.freeBytes) &&
                      volume.totalBytes > 0 &&
                      volume.freeBytes >= 0 &&
                      volume.freeBytes <= volume.totalBytes;
                    const usedGb = hasCapacity ? (volume.totalBytes! - volume.freeBytes!) / 1024 ** 3 : null;
                    const totalGb = hasCapacity ? volume.totalBytes! / 1024 ** 3 : null;
                    const percent = hasCapacity
                      ? Math.round(((volume.totalBytes! - volume.freeBytes!) / volume.totalBytes!) * 100)
                      : null;
                    const health = getDiskCapacityHealth(percent);
                    return (
                      <li key={volume.path} className="storage-volume">
                        <div className="storage-volume-head">
                          <span className="storage-volume-name">{volume.label || volume.path}</span>
                          <span className="storage-volume-size">
                            {hasCapacity ? `${formatCapacity(volume.freeBytes)} livres` : "Capacidade indisponível"}
                          </span>
                        </div>
                        <MiniCapacityBar
                          usedGb={usedGb}
                          totalGb={totalGb}
                          percent={percent}
                          color={diskCapacityColor(health)}
                          ariaLabel={`Ocupação de ${volume.label || volume.path}`}
                        />
                      </li>
                    );
                  })}
                </ul>
              </div>
              <div className="storage-column">
                <span className="storage-column-label">DISCOS FÍSICOS ({physicalDisks.length})</span>
                <ul className="storage-disk-list">
                  {physicalDisks.map(disk => (
                    <li key={disk.index}>
                      <span className="storage-disk-model" title={disk.model}>{disk.model}</span>
                      <span className="storage-disk-size">{formatCapacity(disk.sizeBytes?.value)}</span>
                      <span className="storage-disk-tags">
                        {disk.mediaType && disk.mediaType !== "Não especificado" && (
                          <span className="storage-tag">{disk.mediaType}</span>
                        )}
                        {disk.busType && disk.busType !== "Não especificado" && (
                          <span className="storage-tag">{disk.busType}</span>
                        )}
                        {typeof disk.temperatureCelsius?.value === "number" &&
                          Number.isFinite(disk.temperatureCelsius.value) && (
                            <span className="storage-tag">{metricFmt(disk.temperatureCelsius.value, " °C")}</span>
                          )}
                        {typeof disk.wearPercent === "number" && (
                          <span className="storage-tag">Desgaste {disk.wearPercent}%</span>
                        )}
                        {typeof disk.powerOnHours === "number" && (
                          <span className="storage-tag">{disk.powerOnHours.toLocaleString("pt-BR")} h</span>
                        )}
                        <span className={`storage-tag storage-tag-${diskHealthTagClass(disk.health?.value)}`}>
                          {disk.health?.value || "Saúde indisponível"}
                        </span>
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            </div>
          </section>
        )}

        {selectedGroup === "sensors" && (
          <section className="panel perf-sensors-panel" aria-label="Catálogo de sensores de hardware">
            <div className="perf-section-head">
              <div>
                <h4>Sensores detectados ({filteredSensorList.length})</h4>
                <small>
                  {hardwareTelemetry && hardwareTelemetry.externalProviders.length > 0
                    ? `Provedores ativos: ${hardwareTelemetry.externalProviders.join(", ")}`
                    : "Modo 100% user-mode (sem driver kernel). Zonas térmicas ACPI e GPU são lidas nativamente; para fans da placa-mãe, use HWiNFO ou LibreHardwareMonitor."}
                </small>
              </div>
              <div className="perf-sensor-filters" role="group" aria-label="Filtrar sensores por tipo">
                {[
                  { id: "all", label: "Todos" },
                  { id: "temperature", label: "Temperaturas" },
                  { id: "fan", label: "Ventoinhas" },
                  { id: "power", label: "Energia" },
                  { id: "clock", label: "Clocks" },
                  { id: "voltage", label: "Tensões" },
                ].map(f => (
                  <button
                    key={f.id}
                    type="button"
                    className={`perf-filter-pill ${sensorFilter === f.id ? "is-active" : ""}`}
                    onClick={() => setSensorFilter(f.id)}
                  >
                    {f.label}
                  </button>
                ))}
              </div>
            </div>

            {showFanDiagnosticCard && (
              <div className="perf-fan-empty-card" role="status" aria-label="Diagnóstico de sensores de ventoinha">
                <div className="perf-fan-empty-head">
                  <span className="perf-fan-empty-icon" aria-hidden="true">
                    <Fan size={18} />
                  </span>
                  <div className="perf-fan-empty-title">
                    <strong>Nenhuma ventoinha detectada no momento</strong>
                    <small>
                      O Windows não expõe sensores RPM da placa-mãe nativamente em user-mode.
                    </small>
                  </div>
                  <span className="perf-fan-mode-badge">100% USER-MODE</span>
                </div>
                <div className="perf-fan-empty-body">
                  <div className="perf-fan-checked">
                    <span>Fontes verificadas automaticamente:</span>
                    <ul>
                      <li>
                        <strong>NVIDIA (nvidia-smi)</strong> — rotação (%) da ventoinha da GPU dedicada
                      </li>
                      <li>
                        <strong>HWiNFO Shared Memory</strong> — memória compartilhada{" "}
                        <code>Global\HWiNFO_SENS_SM2</code> e <code>Local\HWiNFO_SENS_SM2</code>
                      </li>
                      <li>
                        <strong>LibreHardwareMonitor / OpenHardwareMonitor</strong> — provedores WMI{" "}
                        <code>root/LibreHardwareMonitor</code> e <code>root/OpenHardwareMonitor</code>
                      </li>
                    </ul>
                  </div>
                  <div className="perf-fan-howto">
                    <strong>Como habilitar leitura de fans da placa-mãe em 1 passo:</strong>
                    <p>
                      Abra o <strong>HWiNFO</strong> com <em>Shared Memory Support</em> ativado ou execute o{" "}
                      <strong>LibreHardwareMonitor</strong> — o Pulse detecta os sensores automaticamente,
                      sem precisar reiniciar.
                    </p>
                  </div>
                </div>
              </div>
            )}

            <div className="perf-sensors-grid">
              {filteredSensorList.map(sensor => {
                const isPinnedTray = preferences.trayMetrics.includes(sensor.id);
                const history = getHistoryForMetric(sensor);
                return (
                  <div key={sensor.id} className="perf-sensor-card">
                    <div className="perf-sensor-card-head">
                      <div>
                        <span className="perf-sensor-name">{sensor.label}</span>
                        <small className="perf-sensor-source">{sensor.source}</small>
                      </div>
                      <div className="perf-sensor-right">
                        <strong>{formatTelemetryNumber(sensor.value, sensor.unit)}</strong>
                        <button
                          type="button"
                          className={`perf-pin-chip ${isPinnedTray ? "is-pinned" : ""}`}
                          disabled={!isPinnedTray && trayFull}
                          onClick={() => toggleTrayMetric(sensor.id)}
                          title={isPinnedTray ? "Remover do Tray" : trayFull ? `Tray cheio (${MAX_TRAY_METRICS}/${MAX_TRAY_METRICS})` : "Fixar no Tray"}
                        >
                          {isPinnedTray ? <PinOff size={11} /> : <Pin size={11} />}
                        </button>
                      </div>
                    </div>
                    <SparklineChart
                      data={history}
                      color={telemetryMetricColor(sensor)}
                      height={26}
                      max={sensor.maxValue ?? 100}
                      autoScale={sensor.maxValue == null && sensor.unit !== "%"}
                      ariaLabel={`Histórico de ${sensor.label}`}
                    />
                  </div>
                );
              })}
              {filteredSensorList.length === 0 && !showFanDiagnosticCard && (
                <p className="perf-empty-hint">
                  Nenhum sensor desta categoria respondeu neste hardware.
                </p>
              )}
            </div>
          </section>
        )}
      </section>
    );
  }

  // ==========================================================================
  // MODO GRADE COMPACTA (cards clicáveis e reordenáveis por drag-and-drop ou botões)
  // ==========================================================================
  const sensorCount = metricsForGroup("sensors").length;

  const renderCardHeader = (
    group: PerformanceCardGroup,
    headingText: string,
    index: number,
  ) => (
    <div className="perf-card-head-row">
      <div className="perf-card-title-cluster">
        <GripVertical size={13} className="perf-card-drag-grip" aria-hidden="true" />
        <p>{headingText}</p>
      </div>
      <div className="perf-card-head-controls">
        <button
          type="button"
          className="perf-card-move-btn"
          disabled={index === 0}
          onClick={event => {
            event.stopPropagation();
            moveCardByOffset(group, -1);
          }}
          aria-label={`Mover ${CARD_ACCESSIBLE_NAME[group]} para trás`}
          title="Mover card para trás"
        >
          <ArrowLeft size={12} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="perf-card-move-btn"
          disabled={index === orderedCardGroups.length - 1}
          onClick={event => {
            event.stopPropagation();
            moveCardByOffset(group, 1);
          }}
          aria-label={`Mover ${CARD_ACCESSIBLE_NAME[group]} para frente`}
          title="Mover card para frente"
        >
          <ArrowRight size={12} aria-hidden="true" />
        </button>
        <span className="perf-card-chevron" aria-hidden="true">
          <ChevronRight size={15} />
        </span>
      </div>
    </div>
  );

  const renderCompactCard = (group: PerformanceCardGroup, index: number) => {
    const isDragging = draggedGroup === group;
    const isDragOver = dragOverGroup === group && draggedGroup !== group;
    const baseProps = {
      "data-group": group,
      role: "button" as const,
      tabIndex: 0,
      "aria-label": `Expandir detalhes de ${CARD_ACCESSIBLE_NAME[group]}`,
      draggable: true,
      onClick: () => openGroup(group),
      onKeyDown: handleCardKeyDown(group),
      onDragStart: handleCardDragStart(group),
      onDragOver: handleCardDragOver(group),
      onDragLeave: handleCardDragLeave(group),
      onDrop: handleCardDrop(group),
      onDragEnd: handleCardDragEnd,
    };
    const stateClasses = `is-interactive ${isDragging ? "is-dragging" : ""} ${isDragOver ? "is-drag-over" : ""}`;

    switch (group) {
      case "cpu":
        return (
          <section key="cpu" className={`panel focused-card ${stateClasses}`} {...baseProps}>
            {renderCardHeader("cpu", "CPU AGORA", index)}
            <strong>{metricFmt(overview?.cpuPercent, "%")}</strong>
            <DetailedResourceChart
              title="USO DA CPU EM TEMPO REAL"
              history={cpuHistory}
              currentValue={metricFmt(overview?.cpuPercent, "%")}
              color="var(--neu-accent)"
              ariaLabel="Gráfico detalhado de CPU"
              loading={cpuChartLoading}
            />
            {renderCardSubMetrics("cpu")}
            <small>
              {cpuSensor
                ? `${metricFmt(cpuSensor.celsius)} °C · ${cpuSensor.label}`
                : hardwareTelemetry?.cpu?.model || "Carga média de processamento"}
            </small>
          </section>
        );

      case "ram":
        return (
          <section key="ram" className={`panel focused-card ${stateClasses}`} {...baseProps}>
            {renderCardHeader("ram", "MEMÓRIA RAM", index)}
            <strong>
              {metricFmt(overview?.memoryUsedGb, " GB")} / {metricFmt(overview?.memoryTotalGb, " GB")}
            </strong>
            <DetailedResourceChart
              title="ALOCAÇÃO DE MEMÓRIA RAM"
              history={ramHistory}
              currentValue={memoryPercent !== undefined ? `${memoryPercent}%` : "—"}
              subValue={
                overview
                  ? `${metricFmt(overview.memoryUsedGb, " GB")} em uso de ${metricFmt(overview.memoryTotalGb, " GB")}`
                  : undefined
              }
              color="var(--neu-signal-secondary)"
              ariaLabel="Gráfico detalhado de Memória RAM"
              loading={ramChartLoading}
            />
            {renderCardSubMetrics("ram")}
            <small>
              {memoryPercent !== undefined ? `${memoryPercent}% em utilização` : "Telemetria de memória"}
            </small>
          </section>
        );

      case "gpu":
        return (
          <section key="gpu" className={`panel focused-card ${stateClasses}`} {...baseProps}>
            {renderCardHeader("gpu", "GPU", index)}
            <strong className="gpu-name" title={hardwareTelemetry?.gpu?.name || performance?.gpu?.value || undefined}>
              {hardwareTelemetry?.gpu?.name || performance?.gpu?.value || "Indisponível"}
            </strong>
            {showGpuChart && (
              <DetailedResourceChart
                title="USO DA GPU EM TEMPO REAL"
                history={gpuHistory}
                currentValue={metricFmt(gpuPercent, "%")}
                color="var(--neu-info)"
                ariaLabel="Gráfico detalhado de GPU"
                loading={gpuChartLoading}
              />
            )}
            {vramTotalGb != null && vramTotalGb > 0 ? (
              <HardwareGaugeBar
                label="VRAM DEDICADA"
                valueText={
                  vramUsedGb != null
                    ? `${formatTelemetryNumber(vramUsedGb, "GB")} / ${formatTelemetryNumber(vramTotalGb, "GB")}`
                    : formatTelemetryNumber(vramTotalGb, "GB")
                }
                percent={vramUsedGb != null ? Math.round((vramUsedGb / vramTotalGb) * 100) : null}
                statusText={hardwareTelemetry?.gpu?.source ?? "DXGI"}
                type="vram"
              />
            ) : legacyVramBytes !== undefined ? (
              <HardwareGaugeBar
                label="VRAM DEDICADA"
                valueText={metricFmt(legacyVramBytes / (1024 * 1024), " MB")}
                percent={null}
                statusText="Memória dedicada"
                type="vram"
              />
            ) : null}
            {renderCardSubMetrics("gpu")}
            <small>
              {gpuSensor
                ? `${metricFmt(gpuSensor.celsius)} °C · ${gpuSensor.label}`
                : performance?.gpu?.unavailableReason || "Controladora gráfica do sistema"}
            </small>
            {otherSensors.length > 0 && (
              <ul className="thermal-sensor-list" aria-label="Outros sensores térmicos">
                {otherSensors.map(sensor => (
                  <li key={sensor.id} className="thermal-sensor-row">
                    <span className="thermal-sensor-label" title={sensor.source}>
                      {sensor.label}
                    </span>
                    <strong className="thermal-sensor-value">{`${metricFmt(sensor.celsius)} °C`}</strong>
                  </li>
                ))}
              </ul>
            )}
          </section>
        );

      case "network":
        return (
          <section key="network" className={`panel focused-card network-card ${stateClasses}`} {...baseProps}>
            {renderCardHeader("network", "REDE", index)}
            <strong className="network-total">
              <span className="network-rate network-rate-down">
                ↓ {formatByteRate(downloadHistory[downloadHistory.length - 1])}
              </span>
              <span className="network-rate network-rate-up">
                ↑ {formatByteRate(uploadHistory[uploadHistory.length - 1])}
              </span>
            </strong>
            <div className="network-charts">
              <div className="network-chart">
                <span className="network-chart-label">DOWNLOAD</span>
                <SparklineChart
                  data={downloadHistory}
                  color="var(--neu-signal-secondary)"
                  height={32}
                  autoScale
                  ariaLabel="Histórico de download da rede"
                />
              </div>
              <div className="network-chart">
                <span className="network-chart-label">UPLOAD</span>
                <SparklineChart
                  data={uploadHistory}
                  color="var(--neu-accent)"
                  height={32}
                  autoScale
                  ariaLabel="Histórico de upload da rede"
                />
              </div>
            </div>
            <ul className="network-adapter-list">
              {networkAdapters.map((adapter, adapterIndex) => (
                <li key={`${adapter.name}-${adapterIndex}`}>
                  <span className={`network-dot ${adapter.status === "Ativo" ? "is-active" : ""}`} aria-hidden="true" />
                  <span className="network-adapter-name" title={adapter.description || adapter.name}>
                    {adapter.name}
                  </span>
                  <span className="network-adapter-rate">
                    ↓ {formatByteRate(adapter.receivedBps?.value)} · ↑ {formatByteRate(adapter.sentBps?.value)}
                  </span>
                </li>
              ))}
              {networkAdapters.length === 0 && (
                <li className="network-adapter-empty">
                  {devices?.adapters?.unavailableReason || "Nenhum adaptador de rede reportado"}
                </li>
              )}
            </ul>
            <small>
              {networkAdapters.length > 0
                ? `${activeAdapterCount} de ${networkAdapters.length} adaptadores ativos`
                : "Adaptadores de rede do sistema"}
            </small>
          </section>
        );

      case "storage": {
        const spanClass = index >= 3 ? "span-2" : "";
        return (
          <section
            key="storage"
            className={`panel focused-card storage-device-card ${spanClass} ${stateClasses}`}
            {...baseProps}
          >
            {renderCardHeader("storage", "ARMAZENAMENTO FÍSICO", index)}
            <strong>
              {capacity.measured
                ? `${formatCapacity(capacity.total - capacity.free)} / ${formatCapacity(capacity.total)}`
                : "Indisponível"}
            </strong>
            {renderCardSubMetrics("storage")}
            <div className="storage-device-body">
              <div className="storage-column">
                <span className="storage-column-label">VOLUMES</span>
                <ul className="storage-volume-list">
                  {volumes.map(volume => {
                    const hasCapacity =
                      typeof volume.totalBytes === "number" &&
                      typeof volume.freeBytes === "number" &&
                      Number.isFinite(volume.totalBytes) &&
                      Number.isFinite(volume.freeBytes) &&
                      volume.totalBytes > 0 &&
                      volume.freeBytes >= 0 &&
                      volume.freeBytes <= volume.totalBytes;
                    const usedGb = hasCapacity ? (volume.totalBytes! - volume.freeBytes!) / 1024 ** 3 : null;
                    const totalGb = hasCapacity ? volume.totalBytes! / 1024 ** 3 : null;
                    const percent = hasCapacity
                      ? Math.round(((volume.totalBytes! - volume.freeBytes!) / volume.totalBytes!) * 100)
                      : null;
                    const health = getDiskCapacityHealth(percent);
                    return (
                      <li key={volume.path} className="storage-volume">
                        <div className="storage-volume-head">
                          <span className="storage-volume-name">{volume.label || volume.path}</span>
                          <span className="storage-volume-size">
                            {hasCapacity ? `${formatCapacity(volume.freeBytes)} livres` : "Capacidade indisponível"}
                          </span>
                        </div>
                        <MiniCapacityBar
                          usedGb={usedGb}
                          totalGb={totalGb}
                          percent={percent}
                          color={diskCapacityColor(health)}
                          ariaLabel={`Ocupação de ${volume.label || volume.path}`}
                        />
                      </li>
                    );
                  })}
                  {volumes.length === 0 && (
                    <li className="storage-empty">Nenhum volume de disco detectado</li>
                  )}
                </ul>
              </div>
              <div className="storage-column">
                <span className="storage-column-label">DISCOS FÍSICOS</span>
                <ul className="storage-disk-list">
                  {physicalDisks.map(disk => (
                    <li key={disk.index}>
                      <span className="storage-disk-model" title={disk.model}>{disk.model}</span>
                      <span className="storage-disk-size">{formatCapacity(disk.sizeBytes?.value)}</span>
                      <span className="storage-disk-tags">
                        {disk.mediaType && disk.mediaType !== "Não especificado" && (
                          <span className="storage-tag">{disk.mediaType}</span>
                        )}
                        {disk.busType && disk.busType !== "Não especificado" && (
                          <span className="storage-tag">{disk.busType}</span>
                        )}
                        {typeof disk.temperatureCelsius?.value === "number" &&
                          Number.isFinite(disk.temperatureCelsius.value) && (
                            <span className="storage-tag">{metricFmt(disk.temperatureCelsius.value, " °C")}</span>
                          )}
                        {typeof disk.wearPercent === "number" && (
                          <span className="storage-tag">Desgaste {disk.wearPercent}%</span>
                        )}
                        <span className={`storage-tag storage-tag-${diskHealthTagClass(disk.health?.value)}`}>
                          {disk.health?.value || "Saúde indisponível"}
                        </span>
                      </span>
                    </li>
                  ))}
                  {physicalDisks.length === 0 && (
                    <li className="storage-empty">
                      {devices?.disks?.unavailableReason || "Nenhum disco físico reportado"}
                    </li>
                  )}
                </ul>
              </div>
            </div>
            <small>
              {physicalDisks.length > 0
                ? `${volumes.length} volume(s) · ${physicalDisks.length} disco(s) físico(s)`
                : "Volumes e discos físicos do sistema"}
            </small>
          </section>
        );
      }
    }
  };

  return (
    <section className="module-dashboard">
      <div className="perf-overview-toolbar">
        <span className="perf-overview-hint">
          Clique em qualquer card para expandir detalhes, arraste para reordenar · Tray ({preferences.trayMetrics.length}/{MAX_TRAY_METRICS})
        </span>
        <button
          type="button"
          className="perf-sensors-shortcut"
          onClick={() => openGroup("sensors")}
        >
          <Thermometer size={14} aria-hidden="true" />
          <span>Sensores de hardware{sensorCount > 0 ? ` (${sensorCount})` : ""}</span>
        </button>
      </div>

      <div className="focused-grid">
        {orderedCardGroups.map((group, index) => renderCompactCard(group, index))}
      </div>
    </section>
  );
}
