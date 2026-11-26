import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  ArrowDown,
  ArrowUp,
  BatteryMedium,
  FlaskConical,
  GripVertical,
  PanelBottom,
  Pin,
  PinOff,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  type LucideIcon,
} from "lucide-react";
import { type KeyboardEvent, useCallback, useEffect, useMemo, useState } from "react";
import { PeripheralBattery } from "./PeripheralBattery";
import {
  DEFAULT_BETA_FEATURES,
  TOP_BAR_WIDGET_CATALOG,
  type BetaFeaturesConfig,
  type DockAppCandidate,
  type DockStatus,
  type TopBarWidgetId,
} from "./betaFeatures";
import "./BetaHub.css";

export {
  DEFAULT_BETA_FEATURES,
  TOP_BAR_WIDGET_CATALOG,
  type BetaFeaturesConfig,
  type DockAppCandidate,
  type DockStatus,
  type TopBarWidgetId,
} from "./betaFeatures";

function reorderItems<T>(items: T[], fromIndex: number, toIndex: number): T[] {
  if (
    fromIndex < 0 ||
    toIndex < 0 ||
    fromIndex >= items.length ||
    toIndex >= items.length ||
    fromIndex === toIndex
  ) {
    return items;
  }
  const next = [...items];
  const [moved] = next.splice(fromIndex, 1);
  next.splice(toIndex, 0, moved);
  return next;
}

function appDisplayNameFromPath(exePath: string): string {
  const trimmed = exePath.trim();
  if (trimmed.toLowerCase().startsWith("shell:appsfolder\\")) {
    const rawId = trimmed.slice("shell:appsfolder\\".length);
    const tail = rawId.split("!").pop() ?? rawId;
    return tail.split("_")[0] || rawId;
  }
  const normalized = trimmed.replace(/\//g, "\\");
  const file = normalized.split("\\").pop() ?? trimmed;
  return file.replace(/\.(exe|lnk)$/i, "") || trimmed;
}

type BetaTabId = "peripheral" | "dock" | "runner";

const BETA_TABS: { id: BetaTabId; label: string; fullLabel: string; Icon: LucideIcon }[] = [
  {
    id: "peripheral",
    label: "PERIFÉRICOS",
    fullLabel: "Bateria de Periféricos Conectados",
    Icon: BatteryMedium,
  },
  {
    id: "dock",
    label: "PULSE SHELL",
    fullLabel: "Pulse Shell: Barra Superior + Dock estilo macOS",
    Icon: PanelBottom,
  },
  {
    id: "runner",
    label: "PULSE RUNNER",
    fullLabel: "Pulse Runner (Launcher Rápido)",
    Icon: FlaskConical,
  },
];

export function BetaHub({
  config,
  onConfigChange,
}: {
  config: BetaFeaturesConfig;
  onConfigChange: (next: BetaFeaturesConfig) => void;
}) {
  const [activeTab, setActiveTab] = useState<BetaTabId>("peripheral");
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [dockStatus, setDockStatus] = useState<DockStatus | null>(null);
  const [candidates, setCandidates] = useState<DockAppCandidate[]>([]);
  const [loadingCandidates, setLoadingCandidates] = useState(false);
  const [quotaOptions, setQuotaOptions] = useState<{ id: string; provider: string }[]>([]);
  const [peripheralOptions, setPeripheralOptions] = useState<{ id: string; name: string }[]>([]);

  const [dragWidget, setDragWidget] = useState<TopBarWidgetId | null>(null);
  const [dragOverWidget, setDragOverWidget] = useState<TopBarWidgetId | null>(null);
  const [dragPinned, setDragPinned] = useState<string | null>(null);
  const [dragOverPinned, setDragOverPinned] = useState<string | null>(null);

  const loadDockStatus = useCallback(async () => {
    try {
      const status = await invoke<DockStatus>("get_dock_status");
      if (status && typeof status.supported === "boolean") {
        setDockStatus(status);
      }
    } catch {
      // Backend antigo ou indisponível nos testes
    }
  }, []);

  const loadCandidates = useCallback(async () => {
    setLoadingCandidates(true);
    try {
      const list = await invoke<DockAppCandidate[]>("list_dock_candidates");
      if (Array.isArray(list)) setCandidates(list);
    } catch {
      // Ignora em plataformas não suportadas
    } finally {
      setLoadingCandidates(false);
    }
  }, []);

  useEffect(() => {
    void loadDockStatus();
  }, [loadDockStatus, config.customDock]);

  useEffect(() => {
    if (config.customDock) {
      void loadCandidates();
    }
  }, [config.customDock, config.dockPinnedApps, loadCandidates]);

  useEffect(() => {
    let disposed = false;
    void Promise.resolve(
      invoke<{ providers?: { id: string; provider: string; disabled?: boolean; status?: string }[] }>(
        "get_subscription_usage",
        { forceRefresh: false },
      ),
    )
      .then(res => {
        if (disposed || !res || !Array.isArray(res.providers)) return;
        const available = res.providers
          .filter(p => p && p.disabled !== true && p.status === "available")
          .map(p => ({ id: p.id, provider: p.provider }));
        setQuotaOptions(available);
      })
      .catch(() => undefined);

    if (config.peripheralBattery) {
      void Promise.resolve(
        invoke<{ devices?: { id: string; name: string; connected?: boolean }[] }>(
          "get_peripheral_batteries",
        ),
      )
        .then(snap => {
          if (disposed || !snap || !Array.isArray(snap.devices)) return;
          setPeripheralOptions(
            snap.devices
              .filter(d => d && typeof d.id === "string" && typeof d.name === "string")
              .map(d => ({ id: d.id, name: d.name })),
          );
        })
        .catch(() => undefined);
    } else {
      setPeripheralOptions([]);
    }
    return () => {
      disposed = true;
    };
  }, [config.peripheralBattery, config.customDock]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<BetaFeaturesConfig>("pulse://beta-features-changed", event => {
      if (!disposed && event.payload) {
        onConfigChange(event.payload);
        void loadDockStatus();
      }
    })
      .then(cleanup => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [loadDockStatus, onConfigChange]);

  const activeWidgetIds = useMemo(() => {
    const validIds = new Set(TOP_BAR_WIDGET_CATALOG.map(item => item.id));
    const source = Array.isArray(config.topBarWidgets)
      ? config.topBarWidgets
      : DEFAULT_BETA_FEATURES.topBarWidgets;
    const seen = new Set<TopBarWidgetId>();
    const ordered: TopBarWidgetId[] = [];
    for (const raw of source) {
      if (validIds.has(raw) && !seen.has(raw)) {
        seen.add(raw);
        ordered.push(raw);
      }
    }
    // Invariante: "tray" ("Ícones de outros apps") é sempre presente e reordenável.
    if (!seen.has("tray")) {
      const insertAt = ordered.findIndex(id => id === "volume" || id === "clock");
      if (insertAt >= 0) ordered.splice(insertAt, 0, "tray");
      else ordered.push("tray");
    }
    return ordered;
  }, [config.topBarWidgets]);

  const activeWidgets = useMemo(() => {
    const byId = new Map(TOP_BAR_WIDGET_CATALOG.map(item => [item.id, item]));
    return activeWidgetIds
      .map(id => byId.get(id))
      .filter((item): item is (typeof TOP_BAR_WIDGET_CATALOG)[number] => Boolean(item));
  }, [activeWidgetIds]);

  const inactiveWidgets = useMemo(() => {
    const activeSet = new Set(activeWidgetIds);
    return TOP_BAR_WIDGET_CATALOG.filter(item => !activeSet.has(item.id));
  }, [activeWidgetIds]);

  const pinnedEntries = useMemo(() => {
    const byLower = new Map(candidates.map(item => [item.exePath.toLowerCase(), item]));
    return config.dockPinnedApps.map(exePath => {
      const match = byLower.get(exePath.toLowerCase());
      return {
        exePath,
        name: match?.name ?? appDisplayNameFromPath(exePath),
        isRunning: match?.isRunning ?? false,
      };
    });
  }, [candidates, config.dockPinnedApps]);

  const unpinnedCandidates = useMemo(
    () => candidates.filter(app => !app.isPinned),
    [candidates],
  );

  const handleToggleFeature = async (
    feature: "peripheralBattery" | "customDock" | "pulseRunner" | "pulseRunnerWinKey",
    enabled: boolean,
  ) => {
    setBusyKey(feature);
    setError(null);
    setStatusMessage(null);
    try {
      const updated = await invoke<BetaFeaturesConfig>("set_beta_feature_enabled", {
        feature,
        enabled,
      });
      onConfigChange(updated);
      await loadDockStatus();
      if (feature === "customDock") {
        setStatusMessage(
          enabled
            ? "Pulse Shell (Barra Superior + Dock) iniciado e área útil reservada."
            : "Pulse Shell encerrado e barra de tarefas nativa do Windows restaurada.",
        );
      } else if (feature === "pulseRunner") {
        setStatusMessage(
          enabled
            ? "Pulse Runner ativado. Pressione a tecla Windows para abrir a busca rápida."
            : "Pulse Runner desativado e comportamento padrão da tecla Windows restaurado.",
        );
      }
    } catch (err) {
      setError(typeof err === "string" ? err : "Não foi possível alterar este recurso Beta.");
    } finally {
      setBusyKey(null);
    }
  };

  const handleDockPatch = async (
    key: string,
    patch: Partial<{
      dockAutoHide: boolean;
      dockMagnification: boolean;
      dockIconSize: number;
      dockPinnedApps: string[];
      topBarWidgets: TopBarWidgetId[];
      topBarQuotaProviders: string[];
      topBarPeripheralDevices: string[];
    }>,
  ) => {
    setBusyKey(key);
    setError(null);
    setStatusMessage(null);
    try {
      const updated = await invoke<BetaFeaturesConfig>("update_dock_settings", { patch });
      onConfigChange(updated);
      await loadDockStatus();
    } catch (err) {
      setError(typeof err === "string" ? err : "Não foi possível atualizar a configuração da Dock.");
    } finally {
      setBusyKey(null);
    }
  };

  const handleTogglePin = async (exePath: string, currentlyPinned: boolean) => {
    const nextPinned = currentlyPinned
      ? config.dockPinnedApps.filter(p => p.toLowerCase() !== exePath.toLowerCase())
      : [...config.dockPinnedApps, exePath];
    await handleDockPatch(`pin:${exePath}`, { dockPinnedApps: nextPinned });
  };

  const handleMovePinned = async (fromIndex: number, toIndex: number) => {
    const nextPinned = reorderItems(config.dockPinnedApps, fromIndex, toIndex);
    if (nextPinned === config.dockPinnedApps) return;
    await handleDockPatch("reorder:pinned", { dockPinnedApps: nextPinned });
  };

  const handleToggleWidget = async (id: TopBarWidgetId, enabled: boolean) => {
    if (id === "tray") return;
    const nextWidgets = enabled
      ? activeWidgetIds.includes(id)
        ? activeWidgetIds
        : [...activeWidgetIds, id]
      : activeWidgetIds.filter(item => item !== id);
    await handleDockPatch(`widget:${id}`, { topBarWidgets: nextWidgets });
  };

  const handleMoveWidget = async (fromIndex: number, toIndex: number) => {
    const nextWidgets = reorderItems(activeWidgetIds, fromIndex, toIndex);
    if (nextWidgets === activeWidgetIds) return;
    await handleDockPatch("reorder:widgets", { topBarWidgets: nextWidgets });
  };

  const selectedQuotaIds = config.topBarQuotaProviders ?? [];
  const selectedPeripheralIds = config.topBarPeripheralDevices ?? [];

  const handleToggleTopBarQuota = async (providerId: string) => {
    const exists = selectedQuotaIds.includes(providerId);
    const next = exists
      ? selectedQuotaIds.filter(id => id !== providerId)
      : [...selectedQuotaIds, providerId];
    await handleDockPatch(`topbar-quota:${providerId}`, { topBarQuotaProviders: next });
  };

  const handleToggleTopBarPeripheral = async (deviceId: string) => {
    const exists = selectedPeripheralIds.includes(deviceId);
    const next = exists
      ? selectedPeripheralIds.filter(id => id !== deviceId)
      : [...selectedPeripheralIds, deviceId];
    await handleDockPatch(`topbar-periph:${deviceId}`, { topBarPeripheralDevices: next });
  };

  const handleRestoreTaskbar = async () => {
    setBusyKey("restore");
    setError(null);
    try {
      const status = await invoke<DockStatus>("restore_windows_taskbar");
      setDockStatus(status);
      onConfigChange({
        ...config,
        customDock: false,
        dockHideNativeTaskbar: false,
      });
      setStatusMessage("Barra de tarefas nativa do Windows restaurada e Pulse Shell desativado.");
    } catch (err) {
      setError(typeof err === "string" ? err : "Falha ao restaurar a barra de tarefas do Windows.");
    } finally {
      setBusyKey(null);
    }
  };

  const dockSupported = dockStatus?.supported ?? true;

  const tabEnabled: Record<BetaTabId, boolean> = {
    peripheral: config.peripheralBattery,
    dock: config.customDock,
    runner: config.pulseRunner,
  };

  const handleTabKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    const currentIndex = BETA_TABS.findIndex(tab => tab.id === activeTab);
    let nextIndex: number | null = null;
    if (event.key === "ArrowRight") nextIndex = (currentIndex + 1) % BETA_TABS.length;
    else if (event.key === "ArrowLeft")
      nextIndex = (currentIndex - 1 + BETA_TABS.length) % BETA_TABS.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = BETA_TABS.length - 1;
    if (nextIndex == null) return;
    event.preventDefault();
    const nextId = BETA_TABS[nextIndex].id;
    setActiveTab(nextId);
    document.getElementById(`beta-tab-${nextId}`)?.focus();
  };

  return (
    <section className="beta-hub" aria-label="Central de Recursos Beta">
      <div className="beta-intro-banner">
        <FlaskConical size={16} aria-hidden="true" />
        <div>
          <strong>Módulos experimentais (opt-in)</strong>
          <p>
            Cada recurso abaixo nasce desligado e consome zero CPU/memória enquanto inativo. Você
            pode ativar ou desligar a qualquer momento sem alterar arquivos do sistema.
          </p>
        </div>
      </div>

      {error && (
        <div className="beta-alert beta-alert-error" role="alert">
          <AlertTriangle size={15} aria-hidden="true" />
          <span>{error}</span>
        </div>
      )}
      {statusMessage && (
        <div className="beta-alert beta-alert-ok" role="status">
          <ShieldCheck size={15} aria-hidden="true" />
          <span>{statusMessage}</span>
        </div>
      )}

      <nav
        className="storage-tabs-bar beta-tabs-bar"
        role="tablist"
        aria-label="Módulos de Recursos Beta"
        onKeyDown={handleTabKeyDown}
      >
        {BETA_TABS.map(tab => {
          const active = activeTab === tab.id;
          const enabled = tabEnabled[tab.id];
          return (
            <button
              key={tab.id}
              id={`beta-tab-${tab.id}`}
              type="button"
              role="tab"
              aria-selected={active}
              aria-controls={`beta-panel-${tab.id}`}
              tabIndex={active ? 0 : -1}
              title={tab.fullLabel}
              className={`storage-tab-btn beta-tab-btn ${active ? "active" : ""}`}
              onClick={() => setActiveTab(tab.id)}
            >
              <tab.Icon size={14} aria-hidden="true" />
              <span>{tab.label}</span>
              <span className={`beta-tab-state ${enabled ? "is-on" : ""}`}>
                {enabled ? "ATIVO" : "INATIVO"}
              </span>
            </button>
          );
        })}
      </nav>

      {/* ---------------- Módulo Beta 1: Bateria de Periféricos ---------------- */}
      {activeTab === "peripheral" && (
        <div
          role="tabpanel"
          id="beta-panel-peripheral"
          aria-labelledby="beta-tab-peripheral"
          className="beta-panel"
        >
          <article className="beta-card" aria-labelledby="beta-peripheral-title">
            <header className="beta-card-header">
              <div className="beta-card-identity">
                <span className="beta-card-icon">
                  <BatteryMedium size={18} aria-hidden="true" />
                </span>
                <div>
                  <div className="beta-title-row">
                    <h3 id="beta-peripheral-title">Bateria de Periféricos Conectados</h3>
                    <span className="beta-badge">BETA · FASE 1</span>
                  </div>
                  <p>
                    Lê o percentual real de mouse, teclado, fones Bluetooth/BLE e controles Xbox sem
                    softwares pesados de fabricante — também aparece no Painel Rápido do Tray.
                  </p>
                </div>
              </div>
              <label className="beta-switch">
                <input
                  type="checkbox"
                  aria-label="Ativar monitor de bateria de periféricos"
                  checked={config.peripheralBattery}
                  disabled={busyKey === "peripheralBattery"}
                  onChange={e => void handleToggleFeature("peripheralBattery", e.target.checked)}
                />
                <span>{config.peripheralBattery ? "ATIVADO" : "DESATIVADO"}</span>
              </label>
            </header>

            {config.peripheralBattery && (
              <div className="beta-card-body">
                <PeripheralBattery />
              </div>
            )}
          </article>
        </div>
      )}

      {/* ---------------- Módulo Beta 2: Pulse Shell (Barra Superior + Dock estilo macOS) ---------------- */}
      {activeTab === "dock" && (
        <div
          role="tabpanel"
          id="beta-panel-dock"
          aria-labelledby="beta-tab-dock"
          className="beta-panel"
        >
          <article className="beta-card" aria-labelledby="beta-dock-title">
            <header className="beta-card-header">
              <div className="beta-card-identity">
                <span className="beta-card-icon">
                  <PanelBottom size={18} aria-hidden="true" />
                </span>
                <div>
                  <div className="beta-title-row">
                    <h3 id="beta-dock-title">Pulse Shell: Barra Superior + Dock estilo macOS</h3>
                    <span className="beta-badge">BETA · WINDOWS</span>
                  </div>
                  <p>
                    Substitui a barra de tarefas do Windows automaticamente por uma Barra Superior com
                    bandeja e widgets fixados + Dock flutuante em Rust + Direct2D, reservando a área
                    útil da tela e mantendo watchdog reversível de restauração.
                  </p>
                </div>
              </div>
              <label className="beta-switch">
                <input
                  type="checkbox"
                  aria-label="Ativar Dock estilo macOS"
                  checked={config.customDock}
                  disabled={!dockSupported || busyKey === "customDock"}
                  onChange={e => void handleToggleFeature("customDock", e.target.checked)}
                />
                <span>{config.customDock ? "ATIVADA" : "DESATIVADA"}</span>
              </label>
            </header>

            <div className="beta-card-body">
              {!dockSupported ? (
                <p className="beta-unavailable" role="status">
                  {dockStatus?.unavailableReason ??
                    "A Dock nativa em Win32 + Direct2D está disponível apenas no Windows 10/11."}
                </p>
              ) : (
                <>
                  <div className="beta-dock-telemetry">
                    <span className={`beta-status-chip ${dockStatus?.running ? "is-on" : ""}`}>
                      {dockStatus?.running ? "DOCK ATIVA" : "INATIVA"}
                    </span>
                    <span className="beta-status-chip">MOTOR: {dockStatus?.engine ?? "Win32 + Direct2D"}</span>
                    {dockStatus?.running && (
                      <span className="beta-status-chip">
                        {dockStatus.itemCount} {dockStatus.itemCount === 1 ? "ITEM" : "ITENS"}
                      </span>
                    )}
                    {dockStatus?.watchdogActive && (
                      <span className="beta-status-chip is-on">WATCHDOG ATIVO</span>
                    )}
                    {dockStatus?.nativeTaskbarHidden && (
                      <span className="beta-status-chip is-warn">TASKBAR NATIVA OCULTA</span>
                    )}
                  </div>

                  {dockStatus?.lastError && (
                    <p className="beta-alert beta-alert-error" role="alert">
                      <AlertTriangle size={14} aria-hidden="true" />
                      <span>{dockStatus.lastError}</span>
                    </p>
                  )}

                  <div className="beta-dock-controls">
                    <label className="beta-option-row">
                      <div>
                        <strong>Auto-ocultar Dock</strong>
                        <span>
                          Recolhe a Dock quando não estiver em uso e libera a área inferior da tela;
                          revela ao encostar na borda inferior.
                        </span>
                      </div>
                      <input
                        type="checkbox"
                        aria-label="Auto-ocultar Dock"
                        checked={config.dockAutoHide}
                        disabled={!config.customDock || busyKey != null}
                        onChange={e =>
                          void handleDockPatch("dockAutoHide", { dockAutoHide: e.target.checked })
                        }
                      />
                    </label>

                    <label className="beta-option-row">
                      <div>
                        <strong>Ampliação de ícones (efeito macOS)</strong>
                        <span>Aumenta suavemente os ícones próximos ao cursor.</span>
                      </div>
                      <input
                        type="checkbox"
                        aria-label="Ampliação de ícones na Dock"
                        checked={config.dockMagnification}
                        disabled={!config.customDock || busyKey != null}
                        onChange={e =>
                          void handleDockPatch("dockMagnification", {
                            dockMagnification: e.target.checked,
                          })
                        }
                      />
                    </label>

                    <div className="beta-slider-row">
                      <div className="beta-slider-label">
                        <strong>Tamanho base dos ícones</strong>
                        <span>{config.dockIconSize} px</span>
                      </div>
                      <input
                        type="range"
                        min={32}
                        max={64}
                        step={2}
                        aria-label="Tamanho base dos ícones da Dock"
                        value={config.dockIconSize}
                        disabled={!config.customDock || busyKey != null}
                        onChange={e =>
                          void handleDockPatch("dockIconSize", {
                            dockIconSize: Number(e.target.value),
                          })
                        }
                      />
                    </div>
                  </div>

                  <div className="beta-dock-actions">
                    <button
                      type="button"
                      className="beta-btn-emergency"
                      disabled={busyKey === "restore"}
                      onClick={() => void handleRestoreTaskbar()}
                    >
                      <RotateCcw size={14} aria-hidden="true" />
                      Restaurar barra do Windows agora
                    </button>
                    <span className="beta-dock-hint">
                      Ativar o Pulse Shell oculta a barra do Windows automaticamente; ao desligar ou em
                      caso de queda, o watchdog restaura a barra e a área útil.
                    </span>
                  </div>

                  {/* ---------------- Widgets fixados da Barra Superior ---------------- */}
                  <div className="beta-pinned-section">
                    <div className="beta-pinned-head">
                      <h4>Barra Superior — Widgets Fixados</h4>
                      <span className="beta-section-meta">Arraste ou use ↑ / ↓ para ordenar</span>
                    </div>

                    {activeWidgets.length === 0 ? (
                      <p className="beta-unavailable">
                        Nenhum widget fixado na Barra Superior. Marque qualquer item abaixo para fixar.
                      </p>
                    ) : (
                      <ul className="beta-order-list" aria-label="Widgets ativos na Barra Superior">
                        {activeWidgets.map((widget, index) => (
                          <li
                            key={widget.id}
                            draggable={busyKey == null}
                            onDragStart={() => setDragWidget(widget.id)}
                            onDragOver={e => {
                              e.preventDefault();
                              if (dragWidget && dragWidget !== widget.id) {
                                setDragOverWidget(widget.id);
                              }
                            }}
                            onDragLeave={() => {
                              if (dragOverWidget === widget.id) setDragOverWidget(null);
                            }}
                            onDrop={e => {
                              e.preventDefault();
                              const fromIdx = dragWidget ? activeWidgetIds.indexOf(dragWidget) : -1;
                              const toIdx = index;
                              setDragWidget(null);
                              setDragOverWidget(null);
                              if (fromIdx >= 0 && fromIdx !== toIdx) {
                                void handleMoveWidget(fromIdx, toIdx);
                              }
                            }}
                            onDragEnd={() => {
                              setDragWidget(null);
                              setDragOverWidget(null);
                            }}
                            className={`beta-order-row ${
                              dragWidget === widget.id ? "is-dragging" : ""
                            } ${dragOverWidget === widget.id ? "is-drag-over" : ""}`}
                          >
                            <div className="beta-order-main">
                              <GripVertical size={14} className="beta-drag-handle" aria-hidden="true" />
                              {widget.locked ? (
                                <span
                                  className="beta-status-chip"
                                  title="Sempre presente na Barra Superior; arraste ou use ↑ / ↓ para posicionar."
                                >
                                  FIXO
                                </span>
                              ) : (
                                <input
                                  type="checkbox"
                                  className="beta-row-check"
                                  aria-label={`Exibir widget ${widget.label} na Barra Superior`}
                                  checked
                                  disabled={busyKey != null}
                                  onChange={e => void handleToggleWidget(widget.id, e.target.checked)}
                                />
                              )}
                              <div className="beta-candidate-info">
                                <strong>{widget.label}</strong>
                                <span>{widget.description}</span>
                              </div>
                            </div>
                            <div className="beta-order-actions">
                              <button
                                type="button"
                                className="beta-move-btn"
                                aria-label={`Mover widget ${widget.label} para cima`}
                                disabled={index === 0 || busyKey != null}
                                onClick={() => void handleMoveWidget(index, index - 1)}
                              >
                                <ArrowUp size={13} aria-hidden="true" />
                              </button>
                              <button
                                type="button"
                                className="beta-move-btn"
                                aria-label={`Mover widget ${widget.label} para baixo`}
                                disabled={index === activeWidgets.length - 1 || busyKey != null}
                                onClick={() => void handleMoveWidget(index, index + 1)}
                              >
                                <ArrowDown size={13} aria-hidden="true" />
                              </button>
                            </div>
                          </li>
                        ))}
                      </ul>
                    )}

                    {inactiveWidgets.length > 0 && (
                      <ul className="beta-order-list beta-order-list-inactive" aria-label="Widgets disponíveis para a Barra Superior">
                        {inactiveWidgets.map(widget => (
                          <li key={widget.id} className="beta-order-row is-inactive">
                            <div className="beta-order-main">
                              <input
                                type="checkbox"
                                className="beta-row-check"
                                aria-label={`Exibir widget ${widget.label} na Barra Superior`}
                                checked={false}
                                disabled={busyKey != null}
                                onChange={e => void handleToggleWidget(widget.id, e.target.checked)}
                              />
                              <div className="beta-candidate-info">
                                <strong>{widget.label}</strong>
                                <span>{widget.description}</span>
                              </div>
                            </div>
                          </li>
                        ))}
                      </ul>
                    )}

                    {activeWidgetIds.includes("quotas") && quotaOptions.length > 0 && (
                      <div className="beta-subpicker" role="group" aria-label="Cotas de IA exibidas na Barra Superior">
                        <div className="beta-subpicker-head">
                          <strong>Cotas de IA fixas na Barra Superior</strong>
                          <span>
                            {selectedQuotaIds.length === 0
                              ? "Automático (menor cota restante)"
                              : `${selectedQuotaIds.length} selecionada(s)`}
                          </span>
                        </div>
                        <div className="beta-chip-list">
                          {quotaOptions.map(q => {
                            const active = selectedQuotaIds.includes(q.id);
                            return (
                              <button
                                key={q.id}
                                type="button"
                                className={`beta-chip-toggle${active ? " is-active" : ""}`}
                                aria-pressed={active}
                                disabled={busyKey != null}
                                onClick={() => void handleToggleTopBarQuota(q.id)}
                              >
                                {q.provider}
                              </button>
                            );
                          })}
                        </div>
                      </div>
                    )}

                    {config.peripheralBattery &&
                      activeWidgetIds.includes("peripherals") &&
                      peripheralOptions.length > 0 && (
                        <div className="beta-subpicker" role="group" aria-label="Periféricos exibidos na Barra Superior">
                          <div className="beta-subpicker-head">
                            <strong>Baterias de periféricos fixas na Barra Superior</strong>
                            <span>
                              {selectedPeripheralIds.length === 0
                                ? "Automático (menor bateria)"
                                : `${selectedPeripheralIds.length} selecionado(s)`}
                            </span>
                          </div>
                          <div className="beta-chip-list">
                            {peripheralOptions.map(dev => {
                              const active = selectedPeripheralIds.includes(dev.id);
                              return (
                                <button
                                  key={dev.id}
                                  type="button"
                                  className={`beta-chip-toggle${active ? " is-active" : ""}`}
                                  aria-pressed={active}
                                  disabled={busyKey != null}
                                  onClick={() => void handleToggleTopBarPeripheral(dev.id)}
                                >
                                  {dev.name}
                                </button>
                              );
                            })}
                          </div>
                        </div>
                      )}
                  </div>

                  {/* ---------------- Aplicativos fixados (ordenáveis) + Janelas abertas ---------------- */}
                  {(config.customDock || pinnedEntries.length > 0) && (
                    <div className="beta-pinned-section">
                      <div className="beta-pinned-head">
                        <h4>Aplicativos fixados na Dock (ordem de exibição)</h4>
                        {config.customDock && (
                          <button
                            type="button"
                            className="beta-btn-small"
                            disabled={loadingCandidates}
                            onClick={() => void loadCandidates()}
                          >
                            <RefreshCw size={12} aria-hidden="true" />
                            Atualizar lista
                          </button>
                        )}
                      </div>

                      {pinnedEntries.length === 0 ? (
                        <p className="beta-unavailable">
                          Nenhum aplicativo fixado ainda. Fixe um app abaixo ou pelo menu de contexto
                          na Dock.
                        </p>
                      ) : (
                        <ul className="beta-order-list" aria-label="Aplicativos fixados na Dock">
                          {pinnedEntries.map((app, index) => (
                            <li
                              key={app.exePath}
                              draggable={busyKey == null}
                              onDragStart={() => setDragPinned(app.exePath)}
                              onDragOver={e => {
                                e.preventDefault();
                                if (dragPinned && dragPinned !== app.exePath) {
                                  setDragOverPinned(app.exePath);
                                }
                              }}
                              onDragLeave={() => {
                                if (dragOverPinned === app.exePath) setDragOverPinned(null);
                              }}
                              onDrop={e => {
                                e.preventDefault();
                                const fromIdx = dragPinned
                                  ? pinnedEntries.findIndex(item => item.exePath === dragPinned)
                                  : -1;
                                const toIdx = index;
                                setDragPinned(null);
                                setDragOverPinned(null);
                                if (fromIdx >= 0 && fromIdx !== toIdx) {
                                  void handleMovePinned(fromIdx, toIdx);
                                }
                              }}
                              onDragEnd={() => {
                                setDragPinned(null);
                                setDragOverPinned(null);
                              }}
                              className={`beta-order-row ${
                                dragPinned === app.exePath ? "is-dragging" : ""
                              } ${dragOverPinned === app.exePath ? "is-drag-over" : ""}`}
                            >
                              <div className="beta-order-main">
                                <GripVertical size={14} className="beta-drag-handle" aria-hidden="true" />
                                <div className="beta-candidate-info">
                                  <strong>{app.name}</strong>
                                  <span title={app.exePath}>
                                    {app.isRunning ? "Em execução" : "Fixado"} · {app.exePath}
                                  </span>
                                </div>
                              </div>
                              <div className="beta-order-actions">
                                <button
                                  type="button"
                                  className="beta-move-btn"
                                  aria-label={`Mover aplicativo ${app.name} para cima`}
                                  disabled={index === 0 || busyKey != null}
                                  onClick={() => void handleMovePinned(index, index - 1)}
                                >
                                  <ArrowUp size={13} aria-hidden="true" />
                                </button>
                                <button
                                  type="button"
                                  className="beta-move-btn"
                                  aria-label={`Mover aplicativo ${app.name} para baixo`}
                                  disabled={index === pinnedEntries.length - 1 || busyKey != null}
                                  onClick={() => void handleMovePinned(index, index + 1)}
                                >
                                  <ArrowDown size={13} aria-hidden="true" />
                                </button>
                                <button
                                  type="button"
                                  className="beta-pin-btn is-pinned"
                                  disabled={busyKey != null}
                                  onClick={() => void handleTogglePin(app.exePath, true)}
                                >
                                  <PinOff size={13} aria-hidden="true" />
                                  Desafixar
                                </button>
                              </div>
                            </li>
                          ))}
                        </ul>
                      )}

                      {config.customDock && (
                        <>
                          <div className="beta-pinned-head beta-subhead">
                            <h4>Janelas abertas disponíveis para fixar</h4>
                          </div>
                          {unpinnedCandidates.length === 0 ? (
                            <p className="beta-unavailable">
                              {candidates.length === 0
                                ? "Nenhuma janela elegível encontrada agora. Abra um aplicativo ou clique em Atualizar lista."
                                : "Todos os aplicativos abertos no momento já estão fixados na Dock."}
                            </p>
                          ) : (
                            <ul className="beta-candidate-list">
                              {unpinnedCandidates.map(app => (
                                <li key={app.exePath} className="beta-candidate-row">
                                  <div className="beta-candidate-info">
                                    <strong>{app.name}</strong>
                                    <span title={app.exePath}>
                                      {app.isRunning ? "Em execução" : "Disponível"} · {app.exePath}
                                    </span>
                                  </div>
                                  <button
                                    type="button"
                                    className="beta-pin-btn"
                                    disabled={busyKey != null}
                                    onClick={() => void handleTogglePin(app.exePath, false)}
                                  >
                                    <Pin size={13} aria-hidden="true" />
                                    Fixar
                                  </button>
                                </li>
                              ))}
                            </ul>
                          )}
                        </>
                      )}
                    </div>
                  )}
                </>
              )}
            </div>
          </article>
        </div>
      )}

      {/* ---------------- Módulo Beta 3: Pulse Runner (Launcher tecla Windows) ---------------- */}
      {activeTab === "runner" && (
        <div
          role="tabpanel"
          id="beta-panel-runner"
          aria-labelledby="beta-tab-runner"
          className="beta-panel"
        >
          <article className="beta-card" aria-labelledby="beta-runner-title">
            <header className="beta-card-header">
              <div className="beta-card-identity">
                <span className="beta-card-icon">
                  <FlaskConical size={18} aria-hidden="true" />
                </span>
                <div>
                  <div className="beta-title-row">
                    <h3 id="beta-runner-title">Pulse Runner (Launcher Rápido)</h3>
                    <span className="beta-badge">BETA · RUST</span>
                  </div>
                  <p>
                    Buscador instantâneo em Rust para aplicativos recentes, programas do Windows,
                    arquivos, módulos do Pulse, ações de sistema e calculadora embutida.
                  </p>
                </div>
              </div>
              <label className="beta-switch">
                <input
                  type="checkbox"
                  aria-label="Ativar Pulse Runner"
                  checked={config.pulseRunner}
                  disabled={busyKey === "pulseRunner"}
                  onChange={e => void handleToggleFeature("pulseRunner", e.target.checked)}
                />
                <span>{config.pulseRunner ? "ATIVADO" : "DESATIVADO"}</span>
              </label>
            </header>

            <div className="beta-card-body">
              <div className="beta-dock-controls">
                <label className="beta-option-row">
                  <div>
                    <strong>Abrir ao apertar a tecla Windows</strong>
                    <span>
                      Intercepta apenas o toque isolado na tecla Windows; combinações como Win+E, Win+R,
                      Win+V, Win+L, Win+D e Win+Shift+S continuam funcionando normalmente.
                    </span>
                  </div>
                  <input
                    type="checkbox"
                    aria-label="Abrir Pulse Runner com a tecla Windows"
                    checked={config.pulseRunnerWinKey}
                    disabled={!config.pulseRunner || busyKey != null}
                    onChange={e => void handleToggleFeature("pulseRunnerWinKey", e.target.checked)}
                  />
                </label>
              </div>
              <div className="beta-dock-actions">
                <button
                  type="button"
                  className="beta-btn-small"
                  onClick={() => void invoke("launcher_toggle").catch(() => undefined)}
                >
                  Abrir Pulse Runner agora
                </button>
                <button
                  type="button"
                  className="beta-btn-small"
                  onClick={() => void invoke("show_files_window", { initialPath: null }).catch(() => undefined)}
                >
                  Abrir Pulse Arquivos (janela independente)
                </button>
              </div>
            </div>
          </article>
        </div>
      )}
    </section>
  );
}
