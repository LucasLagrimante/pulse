import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BetaHub, DEFAULT_BETA_FEATURES, type BetaFeaturesConfig } from "./BetaHub";
import { batteryLevelTone, PeripheralBattery, sanitizeBatteryPercent } from "./PeripheralBattery";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

describe("PeripheralBattery helpers & component", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("sanitizes percentages and rejects null, NaN and out-of-range values", () => {
    expect(sanitizeBatteryPercent(82.4)).toBe(82);
    expect(sanitizeBatteryPercent(0)).toBe(0);
    expect(sanitizeBatteryPercent(100)).toBe(100);
    expect(sanitizeBatteryPercent(null)).toBeNull();
    expect(sanitizeBatteryPercent(undefined)).toBeNull();
    expect(sanitizeBatteryPercent(Number.NaN)).toBeNull();
    expect(sanitizeBatteryPercent(-1)).toBeNull();
    expect(sanitizeBatteryPercent(105)).toBeNull();
  });

  it("assigns semantic battery tones", () => {
    expect(batteryLevelTone(12)).toBe("critical");
    expect(batteryLevelTone(35)).toBe("warning");
    expect(batteryLevelTone(88)).toBe("good");
    expect(batteryLevelTone(null)).toBe("unknown");
  });

  it("renders measured and unavailable devices honestly without fake zeros", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_peripheral_batteries") {
        return Promise.resolve({
          devices: [
            {
              id: "ble-1",
              name: "Logitech MX Master 3S",
              kind: "mouse",
              transport: "bluetooth_le",
              connected: true,
              charging: false,
              batteryPercent: { value: 18, unavailableReason: null },
              source: "Bluetooth LE (GATT 0x180F)",
            },
            {
              id: "xinput-0",
              name: "Controle Xbox #1",
              kind: "controller",
              transport: "xinput",
              connected: true,
              charging: null,
              batteryPercent: {
                value: null,
                unavailableReason: "Conectado via cabo USB (sem telemetria de bateria).",
              },
              source: "XInput (USB)",
            },
          ],
          unavailableReason: null,
          platformNote: "Fase 1 ativa",
          collectedAtUnixMs: Date.now(),
        }) as never;
      }
      return Promise.resolve("") as never;
    });

    render(<PeripheralBattery />);

    await waitFor(() => {
      expect(screen.getByText("Logitech MX Master 3S")).toBeInTheDocument();
    });
    expect(screen.getByText("18%")).toBeInTheDocument();
    expect(screen.getByText("Bateria baixa")).toBeInTheDocument();
    expect(screen.getByText("Controle Xbox #1")).toBeInTheDocument();
    expect(screen.getByText("Indisponível")).toBeInTheDocument();
    expect(screen.getByText("Conectado via cabo USB (sem telemetria de bateria).")).toBeInTheDocument();
  });
});

describe("BetaHub", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("omits redundant taskbar hide toggle, toggles custom dock, and supports emergency restore", async () => {
    let state: BetaFeaturesConfig = { ...DEFAULT_BETA_FEATURES };
    const onConfigChange = vi.fn((next: BetaFeaturesConfig) => {
      state = next;
    });

    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "get_dock_status") {
        return Promise.resolve({
          supported: true,
          running: state.customDock,
          nativeTaskbarHidden: state.customDock,
          watchdogActive: state.customDock,
          itemCount: 3,
          engine: "Win32 + Direct2D (Shell)",
          unavailableReason: null,
          lastError: null,
        }) as never;
      }
      if (command === "set_beta_feature_enabled") {
        const { feature, enabled } = args as { feature: "peripheralBattery" | "customDock"; enabled: boolean };
        state = {
          ...state,
          [feature]: enabled,
          dockHideNativeTaskbar: feature === "customDock" ? enabled : state.dockHideNativeTaskbar,
        };
        return Promise.resolve(state) as never;
      }
      if (command === "restore_windows_taskbar") {
        state = { ...state, customDock: false, dockHideNativeTaskbar: false };
        return Promise.resolve({
          supported: true,
          running: false,
          nativeTaskbarHidden: false,
          watchdogActive: false,
          itemCount: 0,
          engine: "Win32 + Direct2D (Shell)",
          unavailableReason: null,
          lastError: null,
        }) as never;
      }
      if (command === "list_dock_candidates") {
        return Promise.resolve([]) as never;
      }
      return Promise.resolve({}) as never;
    });

    const { rerender } = render(<BetaHub config={state} onConfigChange={onConfigChange} />);

    // Não deve existir checkbox separado para ocultar a barra do Windows
    expect(screen.queryByLabelText("Ocultar barra de tarefas nativa do Windows")).not.toBeInTheDocument();

    const dockToggle = await screen.findByLabelText("Ativar Dock estilo macOS");
    expect(dockToggle).not.toBeChecked();

    fireEvent.click(dockToggle);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_beta_feature_enabled", {
        feature: "customDock",
        enabled: true,
      });
    });

    rerender(<BetaHub config={state} onConfigChange={onConfigChange} />);

    const restoreBtn = screen.getByRole("button", { name: /Restaurar barra do Windows agora/i });
    fireEvent.click(restoreBtn);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("restore_windows_taskbar");
    });
  });

  it("allows toggling and reordering topBarWidgets including quotas and network", async () => {
    let state: BetaFeaturesConfig = {
      ...DEFAULT_BETA_FEATURES,
      customDock: true,
      dockHideNativeTaskbar: true,
    };
    const onConfigChange = vi.fn((next: BetaFeaturesConfig) => {
      state = next;
    });

    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "get_dock_status") {
        return Promise.resolve({
          supported: true,
          running: true,
          nativeTaskbarHidden: true,
          watchdogActive: true,
          itemCount: 2,
          engine: "Win32 + Direct2D (Shell)",
          unavailableReason: null,
          lastError: null,
        }) as never;
      }
      if (command === "list_dock_candidates") {
        return Promise.resolve([]) as never;
      }
      if (command === "update_dock_settings") {
        const { patch } = args as { patch: Partial<BetaFeaturesConfig> };
        state = { ...state, ...patch };
        return Promise.resolve(state) as never;
      }
      return Promise.resolve({}) as never;
    });

    const { rerender } = render(<BetaHub config={state} onConfigChange={onConfigChange} />);

    const networkCheckbox = await screen.findByLabelText("Exibir widget Rede na Barra Superior");
    expect(networkCheckbox).not.toBeChecked();
    expect(screen.getByText("Ícones de outros apps")).toBeInTheDocument();
    expect(screen.queryByLabelText("Exibir widget Ícones de outros apps na Barra Superior")).not.toBeInTheDocument();
    fireEvent.click(networkCheckbox);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_dock_settings", {
        patch: {
          topBarWidgets: ["cpu", "ram", "gpu", "peripherals", "quotas", "tray", "volume", "clock", "network"],
        },
      });
    });

    rerender(<BetaHub config={state} onConfigChange={onConfigChange} />);

    const moveRamUp = screen.getByRole("button", { name: "Mover widget RAM para cima" });
    fireEvent.click(moveRamUp);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_dock_settings", {
        patch: {
          topBarWidgets: ["ram", "cpu", "gpu", "peripherals", "quotas", "tray", "volume", "clock", "network"],
        },
      });
    });
  });

  it("allows reordering dockPinnedApps", async () => {
    let state: BetaFeaturesConfig = {
      ...DEFAULT_BETA_FEATURES,
      customDock: true,
      dockHideNativeTaskbar: true,
      dockPinnedApps: ["C:\\Apps\\Alpha.exe", "C:\\Apps\\Beta.exe"],
    };
    const onConfigChange = vi.fn((next: BetaFeaturesConfig) => {
      state = next;
    });

    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "get_dock_status") {
        return Promise.resolve({
          supported: true,
          running: true,
          nativeTaskbarHidden: true,
          watchdogActive: true,
          itemCount: 2,
          engine: "Win32 + Direct2D (Shell)",
          unavailableReason: null,
          lastError: null,
        }) as never;
      }
      if (command === "list_dock_candidates") {
        return Promise.resolve([
          { name: "Alpha", exePath: "C:\\Apps\\Alpha.exe", isPinned: true, isRunning: true },
          { name: "Beta", exePath: "C:\\Apps\\Beta.exe", isPinned: true, isRunning: false },
          { name: "Gamma", exePath: "C:\\Apps\\Gamma.exe", isPinned: false, isRunning: true },
        ]) as never;
      }
      if (command === "update_dock_settings") {
        const { patch } = args as { patch: Partial<BetaFeaturesConfig> };
        state = { ...state, ...patch };
        return Promise.resolve(state) as never;
      }
      return Promise.resolve({}) as never;
    });

    render(<BetaHub config={state} onConfigChange={onConfigChange} />);

    const moveBetaUp = await screen.findByRole("button", {
      name: "Mover aplicativo Beta para cima",
    });
    fireEvent.click(moveBetaUp);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_dock_settings", {
        patch: {
          dockPinnedApps: ["C:\\Apps\\Beta.exe", "C:\\Apps\\Alpha.exe"],
        },
      });
    });
  });

  it("disables custom dock cleanly on unsupported operating systems", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_dock_status") {
        return Promise.resolve({
          supported: false,
          running: false,
          nativeTaskbarHidden: false,
          watchdogActive: false,
          itemCount: 0,
          engine: "Win32 + Direct2D (Shell)",
          unavailableReason: "A Dock nativa em Win32 + Direct2D está disponível apenas no Windows 10/11.",
          lastError: null,
        }) as never;
      }
      return Promise.resolve({}) as never;
    });

    render(<BetaHub config={DEFAULT_BETA_FEATURES} onConfigChange={vi.fn()} />);

    const dockToggle = await screen.findByLabelText("Ativar Dock estilo macOS");
    await waitFor(() => {
      expect(dockToggle).toBeDisabled();
    });
    expect(
      screen.getByText("A Dock nativa em Win32 + Direct2D está disponível apenas no Windows 10/11."),
    ).toBeInTheDocument();
  });
});
