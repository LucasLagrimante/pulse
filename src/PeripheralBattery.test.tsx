import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TrayFlyout } from "./App";
import {
  PeripheralBattery,
  type PeripheralBatteryDevice,
  type PeripheralBatterySnapshot,
} from "./PeripheralBattery";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

function makeSnapshot(devices: PeripheralBatteryDevice[]): PeripheralBatterySnapshot {
  return {
    devices,
    unavailableReason: null,
    platformNote: null,
    collectedAtUnixMs: 1_700_000_000_000,
  };
}

describe("PeripheralBattery rename flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renames a device inline on Enter, shows hardwareName subtitle, and restores original name", async () => {
    let currentDevices: PeripheralBatteryDevice[] = [
      {
        id: "hid-logi-1",
        name: "Logitech G PRO X Superlight",
        hardwareName: null,
        customName: null,
        kind: "mouse",
        transport: "usb_hid",
        connected: true,
        charging: false,
        batteryPercent: { value: 64, unavailableReason: null },
        source: "Logitech HID++ 2.0",
      },
    ];

    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "get_peripheral_batteries") {
        return Promise.resolve(makeSnapshot(currentDevices)) as never;
      }
      if (command === "set_peripheral_alias") {
        const { deviceId, alias } = args as { deviceId: string; alias: string };
        const trimmed = alias.trim();
        currentDevices = currentDevices.map(dev => {
          if (dev.id !== deviceId) return dev;
          const original = dev.hardwareName ?? dev.name;
          if (!trimmed) {
            return { ...dev, name: original, hardwareName: null, customName: null };
          }
          return { ...dev, name: trimmed, hardwareName: original, customName: trimmed };
        });
        return Promise.resolve(makeSnapshot(currentDevices)) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<PeripheralBattery />);

    await waitFor(() => {
      expect(screen.getByText("Logitech G PRO X Superlight")).toBeInTheDocument();
    });

    // Abre edição inline clicando no botão de lápis
    fireEvent.click(screen.getByRole("button", { name: "Renomear Logitech G PRO X Superlight" }));
    const input = screen.getByRole("textbox", { name: /Novo nome para Logitech G PRO X Superlight/i });
    expect(input).toHaveValue("Logitech G PRO X Superlight");
    expect(screen.queryByRole("button", { name: /Restaurar nome original/i })).not.toBeInTheDocument();

    // Digita novo nome e confirma com Enter
    fireEvent.change(input, { target: { value: "Mouse do Escritório" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_peripheral_alias", {
        deviceId: "hid-logi-1",
        alias: "Mouse do Escritório",
      });
    });

    await waitFor(() => {
      expect(screen.getByText("Mouse do Escritório")).toBeInTheDocument();
    });
    // Subtítulo discreto com o nome original do hardware
    expect(screen.getByText("Logitech G PRO X Superlight")).toBeInTheDocument();

    // Abre novamente e clica em "Restaurar nome original"
    fireEvent.click(screen.getByRole("button", { name: "Renomear Mouse do Escritório" }));
    const restoreBtn = screen.getByRole("button", { name: /Restaurar nome original/i });
    fireEvent.click(restoreBtn);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_peripheral_alias", {
        deviceId: "hid-logi-1",
        alias: "",
      });
    });

    await waitFor(() => {
      expect(screen.queryByText("Mouse do Escritório")).not.toBeInTheDocument();
      expect(screen.getByText("Logitech G PRO X Superlight")).toBeInTheDocument();
    });
  });

  it("cancels inline rename on Escape without invoking set_peripheral_alias", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_peripheral_batteries") {
        return Promise.resolve(
          makeSnapshot([
            {
              id: "ble-1",
              name: "Keychron K2",
              hardwareName: null,
              customName: null,
              kind: "keyboard",
              transport: "bluetooth_le",
              connected: true,
              charging: true,
              batteryPercent: { value: 85, unavailableReason: null },
              source: "Bluetooth LE",
            },
          ]),
        ) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<PeripheralBattery />);

    await waitFor(() => {
      expect(screen.getByText("Keychron K2")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: "Renomear Keychron K2" }));
    const input = screen.getByRole("textbox", { name: /Novo nome para Keychron K2/i });
    fireEvent.change(input, { target: { value: "Outro Nome" } });
    fireEvent.keyDown(input, { key: "Escape" });

    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByText("Keychron K2")).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("set_peripheral_alias", expect.anything());
  });
});

describe("TrayFlyout peripheral battery strip", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function mockFlyoutBase(opts: {
    betaEnabled: boolean;
    devices: PeripheralBatteryDevice[];
  }) {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_beta_features") {
        return Promise.resolve({
          peripheralBattery: opts.betaEnabled,
          customDock: false,
          dockHideNativeTaskbar: false,
          dockAutohide: false,
        }) as never;
      }
      if (command === "get_peripheral_batteries") {
        return Promise.resolve(makeSnapshot(opts.devices)) as never;
      }
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 18,
          memoryUsedGb: 8,
          memoryTotalGb: 16,
          diskUsedPercent: 52,
          topProcesses: [],
        }) as never;
      }
      if (command === "get_audio_devices") {
        return Promise.resolve({ input: [], output: [], unavailableReason: null }) as never;
      }
      if (command === "get_subscription_usage") {
        return Promise.resolve({ providers: [], scanStatus: "idle" }) as never;
      }
      return Promise.resolve(null) as never;
    });
  }

  it("renders compact Periféricos section only for connected devices with measured battery and opens Recursos Beta on click", async () => {
    mockFlyoutBase({
      betaEnabled: true,
      devices: [
        {
          id: "m1",
          name: "Meu Mouse",
          hardwareName: "Logitech G PRO",
          customName: "Meu Mouse",
          kind: "mouse",
          transport: "usb_hid",
          connected: true,
          charging: false,
          batteryPercent: { value: 15, unavailableReason: null },
          source: "Logitech HID++ 2.0",
        },
        {
          id: "h1",
          name: "Cloud Stinger",
          hardwareName: null,
          customName: null,
          kind: "headset",
          transport: "usb_hid",
          connected: true,
          charging: true,
          batteryPercent: { value: 78, unavailableReason: null },
          source: "HyperX",
        },
        {
          id: "c1",
          name: "Controle USB",
          hardwareName: null,
          customName: null,
          kind: "controller",
          transport: "xinput",
          connected: true,
          charging: null,
          batteryPercent: { value: null, unavailableReason: "Via cabo USB" },
          source: "XInput",
        },
        {
          id: "off1",
          name: "Teclado Desligado",
          hardwareName: null,
          customName: null,
          kind: "keyboard",
          transport: "bluetooth_le",
          connected: false,
          charging: false,
          batteryPercent: { value: 40, unavailableReason: null },
          source: "BLE",
        },
      ],
    });

    render(<TrayFlyout onOpenMain={vi.fn()} isForeground />);

    await waitFor(() => {
      expect(screen.getByText("Meu Mouse")).toBeInTheDocument();
    });

    expect(screen.getByText("PERIFÉRICOS")).toBeInTheDocument();
    expect(screen.getByText("15%")).toBeInTheDocument();
    expect(screen.getByText("Cloud Stinger")).toBeInTheDocument();
    expect(screen.getByText("78%")).toBeInTheDocument();
    expect(screen.getByText("⚡")).toBeInTheDocument();
    // Não exibe dispositivos sem bateria medida ou desconectados
    expect(screen.queryByText("Controle USB")).not.toBeInTheDocument();
    expect(screen.queryByText("Teclado Desligado")).not.toBeInTheDocument();

    const mouseChip = screen.getByRole("button", { name: /Meu Mouse, Mouse, 15%, bateria baixa/i });
    expect(mouseChip).toHaveClass("is-low");
    fireEvent.click(mouseChip);

    expect(invoke).toHaveBeenCalledWith("show_main_module", { module: "Recursos Beta" });
  });

  it("hides Periféricos section when beta feature is disabled", async () => {
    mockFlyoutBase({
      betaEnabled: false,
      devices: [
        {
          id: "m1",
          name: "Meu Mouse",
          kind: "mouse",
          transport: "usb_hid",
          connected: true,
          charging: false,
          batteryPercent: { value: 50, unavailableReason: null },
          source: "HID",
        },
      ],
    });

    render(<TrayFlyout onOpenMain={vi.fn()} isForeground />);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("get_beta_features");
    });

    expect(screen.queryByText("PERIFÉRICOS")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("get_peripheral_batteries");
  });
});
