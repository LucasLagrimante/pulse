import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App, {
  CrashBoundary,
  FileList,
  ProcessList,
  ScanFoldersBar,
  ToastNotification,
  TrayFlyout,
  formatByteRate,
  formatCapacity,
  formatFileDate,
  formatFileSize,
  getDiskCapacityHealth,
  groupConsecutiveDebugEntries,
  metric,
} from "./App";
import { DiskAnalyzer, extractTopFiles, formatElapsedSince, getFileExtension } from "./DiskAnalyzer";
import { DiskCleanup } from "./DiskCleanup";

let registeredEventListeners: Record<string, (event: unknown) => void> = {};

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: (event: unknown) => void) => {
    registeredEventListeners[event] = handler;
    return Promise.resolve(() => {
      delete registeredEventListeners[event];
    });
  }),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

describe("Formatters and Components", () => {
  it("groups only consecutive debug entries of the same operation and outcome", () => {
    const entry = (sequence: number, operation: string, outcome = "ok") => ({
      sequence,
      timestampUnixMs: sequence,
      operation,
      category: "telemetry",
      source: "rust-backend",
      durationMs: sequence,
      outcome,
      severity: outcome === "error" ? "error" : "info",
      message: outcome === "error" ? "Falha" : "Concluído",
    });

    const groups = groupConsecutiveDebugEntries([
      entry(1, "subscription_usage_claude"),
      entry(2, "subscription_usage_claude"),
      entry(3, "performance_details"),
      entry(4, "subscription_usage_claude"),
      entry(5, "subscription_usage_claude", "error"),
    ]);

    expect(groups).toHaveLength(4);
    expect(groups[0]?.entries).toHaveLength(2);
    expect(groups[1]?.operation).toBe("performance_details");
    expect(groups[2]?.entries).toHaveLength(1);
    expect(groups[3]?.outcome).toBe("error");
  });

  describe("metric helper", () => {
    it("safely handles numbers, 0, null, undefined and NaN", () => {
      expect(metric(42)).toBe("42");
      expect(metric(0)).toBe("0");
      expect(metric(3.1415, " %")).toBe("3,1 %");
      expect(metric(null)).toBe("—");
      expect(metric(undefined)).toBe("—");
      expect(metric(Number.NaN)).toBe("—");
      expect(metric(null, " GB")).toBe("—");
    });
  });

  describe("disk capacity health", () => {
    it("marks high usage as warning or critical without inventing unavailable status", () => {
      expect(getDiskCapacityHealth(undefined)).toBe("unknown");
      expect(getDiskCapacityHealth(84)).toBe("normal");
      expect(getDiskCapacityHealth(85)).toBe("warning");
      expect(getDiskCapacityHealth(94)).toBe("warning");
      expect(getDiskCapacityHealth(95)).toBe("critical");
    });
  });

  describe("byte rate and capacity formatters", () => {
    it("converts rates and capacities without inventing zero for absent values", () => {
      expect(formatByteRate(null)).toBe("—");
      expect(formatByteRate(undefined)).toBe("—");
      expect(formatByteRate(Number.NaN)).toBe("—");
      expect(formatByteRate(512)).toBe("512 B/s");
      expect(formatByteRate(2048)).toBe("2 KB/s");
      expect(formatByteRate(1_200_000)).toBe("1,1 MB/s");
      expect(formatByteRate(2 * 1024 ** 3)).toBe("2 GB/s");
      expect(formatCapacity(null)).toBe("—");
      expect(formatCapacity(0)).toBe("—");
      expect(formatCapacity(Number.NaN)).toBe("—");
      expect(formatCapacity(500 * 1024 ** 3)).toBe("500 GB");
      expect(formatCapacity(1024 ** 4)).toBe("1 TB");
      expect(formatCapacity(2.5 * 1024 ** 4)).toBe("2,5 TB");
    });
  });

  describe("formatFileSize helper", () => {
    it("handles zero, null, undefined, negative and large byte values", () => {
      expect(formatFileSize(0)).toBe("0 B");
      expect(formatFileSize(null)).toBe("0 B");
      expect(formatFileSize(undefined)).toBe("0 B");
      expect(formatFileSize(-10)).toBe("0 B");
      expect(formatFileSize(Number.NaN)).toBe("0 B");
      expect(formatFileSize(512)).toBe("512 B");
      expect(formatFileSize(204800)).toBe("200,0 KB");
      expect(formatFileSize(1048576)).toBe("1,0 MB");
      expect(formatFileSize(1024 * 1024 * 1024 * 2.5)).toBe("2,5 GB");
    });
  });

  describe("formatFileDate helper", () => {
    it("handles missing or invalid timestamps", () => {
      expect(formatFileDate(0)).toBe("Data indisponível");
      expect(formatFileDate(null)).toBe("Data indisponível");
      expect(formatFileDate(undefined)).toBe("Data indisponível");
      expect(formatFileDate(-1)).toBe("Data indisponível");
      expect(formatFileDate(Number.NaN)).toBe("Data indisponível");
    });

    it("formats relative and absolute dates accurately", () => {
      const now = Date.now();
      expect(formatFileDate(now - 10000)).toBe("Agora mesmo");
      expect(formatFileDate(now - 15 * 60000)).toBe("Há 15 min");

      const yesterday = new Date();
      yesterday.setDate(yesterday.getDate() - 1);
      const yesterdayFormatted = formatFileDate(yesterday.getTime());
      expect(yesterdayFormatted).toMatch(/Ontem às \d{2}:\d{2}/);

      const oldTimestamp = new Date(2023, 0, 15, 14, 30).getTime();
      const oldFormatted = formatFileDate(oldTimestamp);
      expect(oldFormatted).toContain("15/01/2023");
      expect(oldFormatted).toContain("14:30");
    });
  });

  describe("ProcessList component", () => {
    it("shows friendly executable metadata without losing technical identity", () => {
      render(
        <ProcessList
          processes={[
            {
              name: "code.exe",
              id: 42,
              memoryMb: 512,
              displayName: "Visual Studio Code",
              publisher: "Microsoft Corporation",
            },
          ]}
        />,
      );

      expect(screen.getByText("Visual Studio Code")).toBeInTheDocument();
      expect(screen.getByText("code.exe · Microsoft Corporation")).toBeInTheDocument();
      expect(screen.getByText("512 MB")).toBeInTheDocument();
    });
  });

  describe("TrayFlyout standalone component", () => {
    it("renders floating telemetry bar and triggers expected commands", async () => {
      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_system_overview") {
          return Promise.resolve({
            cpuPercent: 18.0,
            memoryUsedGb: 6.0,
            memoryTotalGb: 16.0,
            diskUsedGb: 150.0,
            diskTotalGb: 500.0,
            startupItemCount: 2,
            topProcesses: [
              { name: "chrome.exe", id: 100, memoryMb: 800 },
              { name: "slack.exe", id: 101, memoryMb: 400 },
              { name: "node.exe", id: 102, memoryMb: 250 },
              { name: "discord.exe", id: 103, memoryMb: 220 },
              { name: "steam.exe", id: 104, memoryMb: 180 },
            ],
            sampledAtUnixMs: 1725000000,
            source: "Native Win32",
          }) as never;
        }
        return Promise.resolve() as never;
      });

      const onOpenMain = vi.fn();
      const { container } = render(<TrayFlyout onOpenMain={onOpenMain} />);

      expect(screen.getByRole("complementary", { name: "Painel rápido do tray" })).toBeInTheDocument();
      expect(screen.getByText("PAINEL RÁPIDO")).toBeInTheDocument();
      expect(
        screen.getByRole("img", { name: "Gráfico de histórico de CPU no tray" }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("img", { name: "Gráfico de histórico de Memória RAM no tray" }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("group", { name: "Gráfico de alocação de disco no tray" }),
      ).toBeInTheDocument();
      expect(container.querySelector(".flyout-metrics .meter")).not.toBeInTheDocument();

      await waitFor(() => {
        expect(screen.getAllByText("chrome.exe")[0]).toBeInTheDocument();
        expect(screen.getAllByText("slack.exe")[0]).toBeInTheDocument();
        expect(screen.getAllByText("node.exe")[0]).toBeInTheDocument();
        expect(screen.getAllByText("discord.exe")[0]).toBeInTheDocument();
        expect(screen.getAllByText("steam.exe")[0]).toBeInTheDocument();
      });

      expect(screen.queryByText("Processos indisponíveis nesta coleta.")).not.toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Abrir interface completa" }));
      expect(onOpenMain).toHaveBeenCalled();

      fireEvent.click(screen.getByRole("button", { name: "Fechar painel rápido" }));
      expect(invoke).toHaveBeenCalledWith("hide_tray_flyout");

      fireEvent.click(screen.getByRole("button", { name: "Fechar chrome.exe" }));
      expect(screen.getByRole("dialog", { name: "Confirmar encerramento do processo" })).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "FECHAR AGORA" }));
      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("terminate_process", { id: 100 });
      });

      fireEvent.click(screen.getByRole("button", { name: "Sair do aplicativo" }));
      expect(invoke).toHaveBeenCalledWith("exit_app");
    });

    it("polls only while the tray flyout is in the foreground", async () => {
      vi.clearAllMocks();
      vi.mocked(invoke).mockResolvedValue({
        cpuPercent: 18,
        memoryUsedGb: 6,
        memoryTotalGb: 16,
        diskUsedGb: 150,
        diskTotalGb: 500,
        startupItemCount: 2,
        topProcesses: [],
        sampledAtUnixMs: 1725000000,
        source: "Native Win32",
      } as never);
      const setIntervalSpy = vi.spyOn(window, "setInterval");
      const clearIntervalSpy = vi.spyOn(window, "clearInterval");

      const { rerender } = render(
        <TrayFlyout isForeground={false} onOpenMain={vi.fn()} />,
      );
      expect(invoke).not.toHaveBeenCalledWith("get_system_overview");

      rerender(<TrayFlyout isForeground onOpenMain={vi.fn()} />);
      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("get_system_overview");
      });
      expect(setIntervalSpy).toHaveBeenCalledWith(expect.any(Function), 5000);
      const pollingTimer = setIntervalSpy.mock.results.at(-1)?.value;

      rerender(<TrayFlyout isForeground={false} onOpenMain={vi.fn()} />);
      expect(clearIntervalSpy).toHaveBeenCalledWith(pollingTimer);

      setIntervalSpy.mockRestore();
      clearIntervalSpy.mockRestore();
      vi.clearAllMocks();
    });

    it("shows Windows input/output devices and applies a selected default", async () => {
      vi.clearAllMocks();
      const inventory = {
        input: [{ id: "mic-1", name: "Microfone USB", isDefault: true }],
        output: [{ id: "speaker-1", name: "Alto-falantes USB", isDefault: true }, { id: "speaker-2", name: "Headset", isDefault: false }],
        inputDefaultId: "mic-1",
        outputDefaultId: "speaker-1",
      };
      vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
        if (command === "get_audio_devices") return Promise.resolve(inventory) as never;
        if (command === "set_default_audio_device") {
          expect(args).toEqual({ direction: "output", deviceId: "speaker-2" });
          return Promise.resolve({ ...inventory, outputDefaultId: "speaker-2" }) as never;
        }
        if (command === "get_system_overview") {
          return Promise.resolve({
            cpuPercent: 12,
            memoryUsedGb: 4,
            memoryTotalGb: 16,
            diskUsedGb: 100,
            diskTotalGb: 500,
            topProcesses: [],
          }) as never;
        }
        return Promise.resolve() as never;
      });

      render(<TrayFlyout onOpenMain={vi.fn()} />);
      const output = await screen.findByRole("combobox", { name: "Dispositivo de saída padrão" });
      expect(screen.getByRole("combobox", { name: "Dispositivo de entrada padrão" })).toHaveValue("mic-1");
      expect(output).toHaveValue("speaker-1");
      expect(screen.getByRole("option", { name: "Headset" })).toBeInTheDocument();

      fireEvent.change(output, { target: { value: "speaker-2" } });
      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("set_default_audio_device", {
          direction: "output",
          deviceId: "speaker-2",
        });
        expect(output).toHaveValue("speaker-2");
      });
    });

    it("retains cached AI quota cards during background scan without flickering", async () => {
      const cachedProviders = [
        {
          id: "claude",
          provider: "Claude Code",
          subtitle: "Assinatura Anthropic",
          status: "available",
          windows: [{ id: "session", label: "Sessão", remainingPercent: 85 }],
          source: "Claude Code",
        },
        {
          id: "codex",
          provider: "Codex",
          subtitle: "ChatGPT",
          status: "available",
          windows: [{ id: "session", label: "Sessão", remainingPercent: 90 }],
          source: "Codex",
        },
      ];
      localStorage.setItem("pulse-subscription-providers", JSON.stringify(cachedProviders));

      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_subscription_usage") {
          return Promise.resolve({
            providers: cachedProviders,
            scanStatus: "running",
          }) as never;
        }
        return Promise.resolve() as never;
      });

      render(<TrayFlyout onOpenMain={vi.fn()} />);

      expect(screen.getByText("Claude Code")).toBeInTheDocument();
      expect(screen.getByText("Codex")).toBeInTheDocument();
      expect(screen.queryByText("Consultando fontes locais…")).not.toBeInTheDocument();

      const updatedProviders = [
        ...cachedProviders,
        {
          id: "openrouter",
          provider: "OpenRouter",
          subtitle: "Saldo em Dólares (USD)",
          status: "available",
          creditsRemaining: 4.5,
          windows: [],
          source: "OpenRouter API",
        },
      ];
      await waitFor(() => {
        expect(registeredEventListeners["pulse://subscription-usage-updated"]).toBeDefined();
      });

      act(() => {
        registeredEventListeners["pulse://subscription-usage-updated"]?.({
          payload: {
            providers: updatedProviders,
            scanStatus: "ready",
          },
        });
      });

      expect(await screen.findByText("OpenRouter")).toBeInTheDocument();
      expect(screen.getAllByText("$ 4,50").length).toBeGreaterThanOrEqual(1);
      expect(screen.getByText("Claude Code")).toBeInTheDocument();
    });

    it("hides disabled AI providers from the tray flyout", async () => {
      const providers = [
        {
          id: "claude",
          provider: "Claude Code",
          subtitle: "Assinatura Anthropic",
          status: "available",
          disabled: true,
          windows: [{ id: "session", label: "Sessão", remainingPercent: 85 }],
          source: "Claude Code",
        },
        {
          id: "codex",
          provider: "Codex",
          subtitle: "ChatGPT",
          status: "available",
          windows: [{ id: "session", label: "Sessão", remainingPercent: 90 }],
          source: "Codex",
        },
      ];
      localStorage.setItem("pulse-subscription-providers", JSON.stringify(providers));

      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_subscription_usage") {
          return Promise.resolve({ providers, scanStatus: "ready" }) as never;
        }
        return Promise.resolve() as never;
      });

      render(<TrayFlyout onOpenMain={vi.fn()} />);

      expect(await screen.findByText("Codex")).toBeInTheDocument();
      expect(screen.queryByText("Claude Code")).not.toBeInTheDocument();
      expect(screen.getByText("1 DISPONÍVEIS")).toBeInTheDocument();
    });
  });

  describe("DiskAnalyzer scan reconciliation", () => {
    it("shows loading feedback inside both disk panels while restoring cached data", async () => {
      let resolveScanState!: (state: unknown) => void;
      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_system_drives") {
          return Promise.resolve([{ path: "C:\\", label: "Disco Local (C:)" }]) as never;
        }
        if (command === "get_disk_scan_state") {
          return new Promise(resolve => { resolveScanState = resolve; }) as never;
        }
        return Promise.resolve(null) as never;
      });

      render(<DiskAnalyzer />);

      expect(screen.getByText("CARREGANDO ANÁLISE LOCAL")).toBeInTheDocument();
      expect(screen.getByText("PREPARANDO MAPA DE BLOCOS")).toBeInTheDocument();

      resolveScanState({ isScanning: false, progress: null, result: null, error: null });
      await waitFor(() => {
        expect(screen.queryByText("CARREGANDO ANÁLISE LOCAL")).not.toBeInTheDocument();
        expect(screen.getByText("Nenhuma varredura realizada")).toBeInTheDocument();
      });
    });

    it("identifies a restored disk analysis as cache with its completion time", async () => {
      const completedAtUnixMs = Date.now() - 2 * 24 * 60 * 60 * 1000;
      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_system_drives") {
          return Promise.resolve([{ path: "C:\\", label: "Disco Local (C:)", totalBytes: 1000, freeBytes: 500 }]) as never;
        }
        if (command === "get_cached_disk_scan") {
          return Promise.resolve({
            completedAtUnixMs,
            durationMs: 1250,
            result: { id: "C:\\", name: "C:\\", path: "C:\\", sizeBytes: 500, fileCount: 1, dirCount: 0, isDir: true, children: [] },
          }) as never;
        }
        if (command === "get_disk_scan_state") {
          return Promise.resolve({ isScanning: false, progress: null, result: null, error: null }) as never;
        }
        return Promise.resolve() as never;
      });
      render(<DiskAnalyzer />);

      await waitFor(() => {
        expect(screen.getByText("RESULTADO EM CACHE")).toBeInTheDocument();
        expect(screen.getByText("há 2 dias")).toBeInTheDocument();
        expect(screen.getByText(/duração: 1 s/)).toBeInTheDocument();
      });
    });

    it("formats elapsed cache time in a human-readable form", () => {
      expect(formatElapsedSince(1_000, 1_000 + 3 * 24 * 60 * 60 * 1000)).toBe("há 3 dias");
    });

    it("explains when a cached scan was cancelled", async () => {
      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_system_drives") {
          return Promise.resolve([{ path: "C:\\", label: "Disco Local (C:)" }]) as never;
        }
        if (command === "get_cached_disk_scan") {
          return Promise.resolve({
            completedAtUnixMs: Date.now(),
            durationMs: 88,
            isPartial: true,
            itemsScanned: 12103,
            stopReason: "cancelled",
            result: { id: "C:\\", name: "C:\\", path: "C:\\", sizeBytes: 500, fileCount: 1, dirCount: 1, isDir: true, children: [] },
          }) as never;
        }
        if (command === "get_disk_scan_state") {
          return Promise.resolve({ isScanning: false, progress: null, result: null, error: null }) as never;
        }
        return Promise.resolve() as never;
      });

      render(<DiskAnalyzer />);

      await waitFor(() => {
        expect(screen.getByText("VARREDURA CANCELADA EM CACHE")).toBeInTheDocument();
        expect(screen.getByText("12.103 itens examinados")).toBeInTheDocument();
        expect(screen.getByText("interrompida pelo usuário")).toBeInTheDocument();
      });
    });

    it("restores an active scan when the module is mounted again", async () => {
      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_system_drives") {
          return Promise.resolve([
            { path: "C:\\", label: "Disco Local (C:)", totalBytes: 1000, freeBytes: 500 },
          ]) as never;
        }
        if (command === "get_custom_scan_folders") {
          return Promise.resolve([]) as never;
        }
        if (command === "get_disk_scan_state") {
          return Promise.resolve({
            isScanning: true,
            progress: {
              itemsScanned: 17,
              totalBytesScanned: 4096,
              currentPath: "C:\\Users\\lucas",
            },
            result: null,
            error: null,
          }) as never;
        }
        return Promise.resolve() as never;
      });
      vi.mocked(invoke).mockClear();

      render(<DiskAnalyzer />);

      await waitFor(() => {
        expect(screen.getByRole("status")).toHaveTextContent("17 itens examinados");
        expect(screen.getByRole("button", { name: "PARAR VARREDURA" })).toBeInTheDocument();
      });
      expect(invoke).not.toHaveBeenCalledWith("get_cached_disk_scan");
    });

    it("renders partial tree data while the native scan is still running", async () => {
      registeredEventListeners = {};
      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "get_system_drives") {
          return Promise.resolve([{ path: "C:\\", label: "Disco Local (C:)" }]) as never;
        }
        if (command === "get_cached_disk_scan") return Promise.resolve(null) as never;
        if (command === "get_disk_scan_state") {
          return Promise.resolve({ isScanning: false, progress: null, result: null, error: null }) as never;
        }
        if (command === "start_disk_scan") {
          return new Promise(() => undefined) as never;
        }
        return Promise.resolve() as never;
      });

      render(<DiskAnalyzer />);
      await waitFor(() => {
        expect(registeredEventListeners["pulse://disk-scan-progress"]).toBeDefined();
        expect(screen.getByRole("button", { name: "ESCANEAR DISCO" })).toBeInTheDocument();
      });

      fireEvent.click(screen.getByRole("button", { name: "ESCANEAR DISCO" }));
      act(() => {
        registeredEventListeners["pulse://disk-scan-progress"]({
          payload: {
            itemsScanned: 25,
            totalBytesScanned: 4096,
            currentPath: "C:\\Users",
            partialResult: {
              id: "C:\\",
              name: "C:\\",
              path: "C:\\",
              sizeBytes: 4096,
              fileCount: 1,
              dirCount: 2,
              isDir: true,
              children: [{
                id: "C:\\Users",
                name: "Users",
                path: "C:\\Users",
                sizeBytes: 4096,
                fileCount: 1,
                dirCount: 1,
                isDir: true,
                children: [],
              }],
            },
          },
        });
      });

      expect(screen.getByRole("status")).toHaveTextContent("25 itens examinados");
      expect(screen.getAllByText("Users").length).toBeGreaterThanOrEqual(2);
      expect(screen.getByRole("button", { name: "PARAR VARREDURA" })).toBeInTheDocument();
    });
  });

  describe("DiskCleanup", () => {
    it("analyzes, lets the user revise categories and requires confirmation", async () => {
      vi.mocked(invoke).mockImplementation((command: string) => {
        if (command === "analyze_disk_cleanup") {
          return Promise.resolve({
            scannedAtUnixMs: Date.now(),
            categories: [
              {
                id: "user-temp",
                name: "Arquivos temporários do usuário",
                description: "Temporários seguros",
                recommended: true,
                availableBytes: 2 * 1024 * 1024,
                itemCount: 3,
                isPartial: false,
                largestItems: [{ path: "C:\\Temp\\cache.bin", name: "cache.bin", sizeBytes: 1024, isDir: false }],
              },
              {
                id: "crash-dumps",
                name: "Relatórios e dumps de falha",
                description: "Diagnósticos para revisão",
                recommended: false,
                availableBytes: 1024 * 1024,
                itemCount: 1,
                isPartial: false,
                largestItems: [],
              },
              {
                id: "recycle-bin",
                name: "Lixeira do Windows",
                description: "Exclusão permanente para revisão",
                recommended: false,
                availableBytes: 4 * 1024 * 1024,
                itemCount: 2,
                isPartial: false,
                largestItems: [],
              },
            ],
          }) as never;
        }
        if (command === "clean_disk_categories") {
          return Promise.resolve({ reclaimedBytes: 3 * 1024 * 1024, deletedFiles: 4, deletedDirectories: 1, skippedItems: 0 }) as never;
        }
        return Promise.resolve() as never;
      });

      render(<DiskCleanup />);
      fireEvent.click(screen.getByRole("button", { name: "ANALISAR AGORA" }));

      expect(await screen.findByText("Arquivos temporários do usuário")).toBeInTheDocument();
      expect(screen.getByText("cache.bin")).toBeInTheDocument();
      expect(screen.getByRole("checkbox", { name: /Arquivos temporários/ })).toBeChecked();
      expect(screen.getByRole("checkbox", { name: /Relatórios e dumps/ })).not.toBeChecked();
      expect(screen.getByRole("checkbox", { name: /Lixeira do Windows/ })).not.toBeChecked();

      fireEvent.click(screen.getByRole("checkbox", { name: /Relatórios e dumps/ }));
      fireEvent.click(screen.getByRole("button", { name: "REVISAR E LIMPAR" }));
      expect(screen.getByRole("dialog")).toHaveTextContent("Liberar até 3,0 MB?");

      fireEvent.click(screen.getByRole("button", { name: "CONFIRMAR LIMPEZA" }));
      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("clean_disk_categories", {
          categoryIds: expect.arrayContaining(["user-temp", "crash-dumps"]),
        });
      });
    });
  });
});

describe("Pulse application shell", () => {
  beforeEach(() => {
    registeredEventListeners = {};
    vi.clearAllMocks();
    vi.mocked(invoke).mockImplementation((command: string) => {
      switch (command) {
        case "get_current_window_label":
          return Promise.resolve("main") as never;
        case "is_current_window_foreground":
          return Promise.resolve(true) as never;
        case "get_system_overview":
          return Promise.resolve({
            cpuPercent: 12.5,
            memoryUsedGb: 8.2,
            memoryTotalGb: 16.0,
            diskUsedGb: 120.0,
            diskTotalGb: 500.0,
            startupItemCount: 4,
            topProcesses: [
              { name: "chrome.exe", id: 1001, memoryMb: 1024 },
              { name: "code.exe", id: 1002, memoryMb: 512 },
              { name: "vmmemWSL", id: 1003, memoryMb: 384 },
            ],
            sampledAtUnixMs: 1725000000,
            source: "Windows CIM",
          }) as never;
        case "get_performance_details":
          return Promise.resolve({
            gpu: { value: "NVIDIA GeForce RTX 4070" },
            vramBytes: { value: 12 * 1024 * 1024 * 1024 },
            temperatureCelsius: { value: 45.5 },
          }) as never;
        case "get_performance_devices":
          return Promise.resolve({
            adapters: {
              value: [
                {
                  name: "Ethernet",
                  description: "Realtek PCIe GbE Family Controller",
                  status: "Ativo",
                  hardwareInterface: true,
                  linkSpeedBps: { value: 1_000_000_000 },
                  receivedBps: { value: 1.2 * 1024 ** 2 },
                  sentBps: { value: 256 * 1024 },
                },
                {
                  name: "Wi-Fi",
                  description: "Intel Wi-Fi 6E AX211",
                  status: "Inativo",
                  hardwareInterface: true,
                  linkSpeedBps: { value: 0 },
                  receivedBps: { value: null, unavailableReason: "Aguardando a primeira janela de medição" },
                  sentBps: { value: null, unavailableReason: "Aguardando a primeira janela de medição" },
                },
              ],
              unavailableReason: null,
            },
            disks: {
              value: [
                {
                  index: 0,
                  model: "Samsung SSD 990 PRO 1TB",
                  mediaType: "SSD",
                  busType: "NVMe",
                  sizeBytes: { value: 1_000_204_886_016 },
                  health: { value: "Saudável" },
                  temperatureCelsius: { value: 38 },
                },
              ],
              unavailableReason: null,
            },
            volumes: [
              {
                path: "C:\\",
                label: "Disco Local (C:)",
                totalBytes: 500 * 1024 ** 3,
                freeBytes: 150 * 1024 ** 3,
              },
            ],
          }) as never;
        case "list_recent_files":
        case "refresh_recent_files":
          return Promise.resolve([
            {
              name: "relatorio.pdf",
              path: "C:\\Users\\lucas\\Documents\\relatorio.pdf",
              category: "document",
              sizeBytes: 204800,
              modifiedAtUnixMs: 1725000000000,
            },
            {
              name: "foto.png",
              path: "C:\\Users\\lucas\\Pictures\\foto.png",
              category: "image",
              sizeBytes: 1048576,
              modifiedAtUnixMs: 1725005000000,
            },
            {
              name: "backup.zip",
              path: "C:\\Users\\lucas\\Downloads\\backup.zip",
              category: "archive",
              sizeBytes: 52428800,
              modifiedAtUnixMs: 1724900000000,
            },
          ]) as never;
        case "list_startup_items":
          return Promise.resolve([
            {
              name: "OneDrive",
              command: "onedrive.exe /background",
              origin: "HKCU\\Run",
              enabled: true,
              canChange: true,
            },
          ]) as never;
        case "set_startup_item_enabled":
          return Promise.resolve({
            name: "OneDrive",
            command: "onedrive.exe /background",
            origin: "HKCU\\Run",
            enabled: false,
            canChange: true,
          }) as never;
        case "get_startup_registration":
          return Promise.resolve({ enabled: false }) as never;
        case "set_startup_registration":
          return Promise.resolve({ enabled: true }) as never;
        case "get_debug_mode":
          return Promise.resolve(false) as never;
        case "set_debug_mode":
          return Promise.resolve(true) as never;
        case "get_debug_log":
          return Promise.resolve([
            {
              timestampUnixMs: 1725000000,
              operation: "system_overview",
              durationMs: 45,
              outcome: "ok",
              message: "Concluído",
            },
          ]) as never;
        case "get_custom_scan_folders":
          return Promise.resolve(["D:\\Images\\Screenshots"]) as never;
        case "add_custom_scan_folder":
          return Promise.resolve(["D:\\Images\\Screenshots", "E:\\Projetos"]) as never;
        case "remove_custom_scan_folder":
          return Promise.resolve([]) as never;
        case "get_system_drives":
          return Promise.resolve([
            {
              path: "C:\\",
              label: "Disco Local (C:)",
              totalBytes: 500 * 1024 * 1024 * 1024,
              freeBytes: 150 * 1024 * 1024 * 1024,
            },
          ]) as never;
        case "start_disk_scan":
          return Promise.resolve({
            id: "C:\\",
            name: "C:\\",
            path: "C:\\",
            sizeBytes: 350 * 1024 * 1024 * 1024,
            fileCount: 45000,
            dirCount: 8200,
            isDir: true,
            modifiedAtUnixMs: 1725000000000,
            children: [
              {
                id: "C:\\Program Files",
                name: "Program Files",
                path: "C:\\Program Files",
                sizeBytes: 200 * 1024 * 1024 * 1024,
                fileCount: 25000,
                dirCount: 4000,
                isDir: true,
                modifiedAtUnixMs: 1725000000000,
                children: [
                  {
                    id: "C:\\Program Files\\Steam",
                    name: "Steam",
                    path: "C:\\Program Files\\Steam",
                    sizeBytes: 150 * 1024 * 1024 * 1024,
                    fileCount: 18000,
                    dirCount: 2000,
                    isDir: true,
                    modifiedAtUnixMs: 1725000000000,
                    children: [],
                  },
                ],
              },
              {
                id: "C:\\pagefile.sys",
                name: "pagefile.sys",
                path: "C:\\pagefile.sys",
                sizeBytes: 16 * 1024 * 1024 * 1024,
                fileCount: 1,
                dirCount: 0,
                isDir: false,
                modifiedAtUnixMs: 1725000000000,
                children: [],
              },
            ],
          }) as never;
        case "cancel_disk_scan":
          return Promise.resolve(true) as never;
        case "open_disk_item":
          return Promise.resolve() as never;
        case "minimize_to_tray":
        case "terminate_process":
        case "exit_app":
        case "show_main_window":
        case "hide_tray_flyout":
        case "open_recent_file":
        case "open_recent_file_location":
          return Promise.resolve() as never;
        default:
          return Promise.resolve() as never;
      }
    });
  });

  it("defers collection in the tray and refreshes as soon as the main window returns", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("main") as never;
      if (command === "is_current_window_foreground") return Promise.resolve(false) as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 12,
          memoryUsedGb: 8,
          memoryTotalGb: 16,
          diskUsedGb: 120,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [],
          sampledAtUnixMs: 1725000000,
          source: "Native Win32",
        }) as never;
      }
      if (command === "get_startup_registration") {
        return Promise.resolve({ enabled: false }) as never;
      }
      if (command === "get_performance_details") return Promise.resolve({}) as never;
      return Promise.resolve([]) as never;
    });

    const clearTimeoutSpy = vi.spyOn(window, "clearTimeout");
    render(<App />);

    await waitFor(() => {
      expect(registeredEventListeners["pulse://foreground-changed"]).toBeDefined();
    });
    expect(invoke).not.toHaveBeenCalledWith("get_system_overview");

    act(() => {
      registeredEventListeners["pulse://foreground-changed"]({ payload: true });
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("get_system_overview");
    });

    clearTimeoutSpy.mockClear();
    act(() => {
      registeredEventListeners["pulse://foreground-changed"]({ payload: false });
    });
    expect(clearTimeoutSpy).toHaveBeenCalled();
    clearTimeoutSpy.mockRestore();
  });

  it("renders the product identity, primary areas and unified dashboard", async () => {
    const { container } = render(<App />);

    expect(screen.getByRole("heading", { name: "Pulse" })).toBeInTheDocument();
    expect(screen.getByText("POR LUCAS LAGRIMANTE")).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Visão geral",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Atividade",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Desempenho",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Armazenamento",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Inicialização e segurança",
    );

    expect(screen.getByRole("heading", { name: "Painel de telemetria local" })).toBeInTheDocument();
    expect(screen.getByText("CPU agora")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Gráfico de tendência de CPU" })).toBeInTheDocument();
    expect(screen.getByText("Memória RAM")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Gráfico de tendência de memória RAM" })).toBeInTheDocument();
    expect(screen.getByText("Disco C:")).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Gráfico de ocupação de disco" })).toBeInTheDocument();
    expect(screen.getByText("Inicialização")).toBeInTheDocument();
    expect(container.querySelector(".metric-grid .meter")).not.toBeInTheDocument();

    await waitFor(() => {
      expect(screen.getAllByText("chrome.exe")[0]).toBeInTheDocument();
      expect(screen.getByText("relatorio.pdf")).toBeInTheDocument();
    });

    await waitFor(() => {
      const startupLabel = screen.getByText("Itens detectados no Windows");
      expect(startupLabel.parentElement?.querySelector("strong")?.textContent).toBe("1");
      expect(startupLabel.parentElement).toHaveTextContent("1 ATIVO");
    });
  });

  it("shows discreet authorship details and opens official contact links", async () => {
    render(<App />);

    await waitFor(() => expect(screen.getAllByText("chrome.exe")[0]).toBeInTheDocument());
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Sobre o Pulse" }));
    });

    expect(screen.getByRole("heading", { name: "Sobre o Pulse." })).toBeInTheDocument();
    expect(screen.getByText("Lucas Lagrimante")).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /lucaslagrimante\.github\.io/ }));
      fireEvent.click(screen.getByRole("button", { name: /llagrimante@gmail\.com/ }));
    });

    expect(openUrl).toHaveBeenCalledWith("https://lucaslagrimante.github.io/");
    expect(openUrl).toHaveBeenCalledWith("mailto:llagrimante@gmail.com");
  });

  it("opens the corresponding module from a telemetry card", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getAllByText("chrome.exe")[0]).toBeInTheDocument();
      expect(screen.getByText("relatorio.pdf")).toBeInTheDocument();
    });

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Abrir desempenho — CPU" }));
    });

    expect(screen.getByRole("button", { name: /Desempenho/ })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(
      screen.getByRole("heading", { name: "Desempenho — recursos em uso" }),
    ).toBeInTheDocument();
    expect(screen.getByText("NVIDIA GeForce RTX 4070")).toHaveClass("gpu-name");
    expect(screen.getByText("NVIDIA GeForce RTX 4070")).toHaveAttribute(
      "title",
      "NVIDIA GeForce RTX 4070",
    );
  });

  it("opens Inicialização e segurança from its telemetry card", async () => {
    render(<App />);

    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Abrir inicialização e segurança" }),
      );
    });

    expect(screen.getByRole("button", { name: /Inicialização e segurança/ })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Itens de Inicialização" })).toBeInTheDocument();
    });
  });

  it("hides unavailable hardware metrics instead of rendering empty gauges", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      switch (command) {
        case "get_current_window_label":
          return Promise.resolve("main") as never;
        case "is_current_window_foreground":
          return Promise.resolve(true) as never;
        case "get_performance_details":
          return Promise.resolve({
            gpu: { value: null, unavailableReason: "Driver genérico do Windows" },
            vramBytes: { value: null, unavailableReason: "VRAM não exposta pelo driver" },
            temperatureCelsius: { value: null, unavailableReason: "Temperatura não exposta pelo driver WMI" },
          }) as never;
        case "get_system_overview":
          return Promise.resolve({
            cpuPercent: 5,
            gpuPercent: null,
            memoryUsedGb: 4,
            memoryTotalGb: 16,
            diskUsedGb: 100,
            diskTotalGb: 500,
            startupItemCount: 1,
            topProcesses: [],
          }) as never;
        default:
          return Promise.resolve() as never;
      }
    });

    const { container } = render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Desempenho/ }));

    await waitFor(() => {
      expect(screen.getByText("CPU AGORA")).toBeInTheDocument();
      expect(screen.getByRole("region", { name: "Gráfico detalhado de CPU" })).toBeInTheDocument();
      expect(screen.getByRole("region", { name: "Gráfico detalhado de Memória RAM" })).toBeInTheDocument();
      expect(screen.getByText("Driver genérico do Windows")).toBeInTheDocument();
    });

    expect(screen.queryByText("CONTROLADORA GRÁFICA")).not.toBeInTheDocument();
    expect(screen.queryByText("Utilização não exposta por esta fonte")).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Gráfico detalhado de GPU" })).not.toBeInTheDocument();
    expect(screen.queryByText("VRAM / TEMPERATURA")).not.toBeInTheDocument();
    expect(screen.queryByText("VRAM DEDICADA")).not.toBeInTheDocument();
    expect(screen.queryByText("TEMPERATURA")).not.toBeInTheDocument();
    expect(screen.queryByText("VRAM não exposta pelo driver")).not.toBeInTheDocument();
    expect(screen.queryByText("Temperatura não exposta pelo driver WMI")).not.toBeInTheDocument();

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(container.querySelector(".focused-grid .meter")).not.toBeInTheDocument();
  });

  it("renders live GPU utilization from engine counters when the driver exposes them", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      switch (command) {
        case "get_current_window_label":
          return Promise.resolve("main") as never;
        case "is_current_window_foreground":
          return Promise.resolve(true) as never;
        case "get_performance_details":
          return Promise.resolve({
            gpu: { value: "NVIDIA GeForce RTX 4070 SUPER" },
          }) as never;
        case "get_system_overview":
          return Promise.resolve({
            cpuPercent: 10,
            gpuPercent: 37.5,
            memoryUsedGb: 8,
            memoryTotalGb: 16,
            diskUsedGb: 100,
            diskTotalGb: 500,
            startupItemCount: 1,
            topProcesses: [],
          }) as never;
        default:
          return Promise.resolve() as never;
      }
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Desempenho/ }));

    await waitFor(() => {
      expect(screen.getByText("USO DA GPU EM TEMPO REAL")).toBeInTheDocument();
      expect(screen.getByRole("region", { name: "Gráfico detalhado de GPU" })).toBeInTheDocument();
      expect(screen.getByText("37,5%")).toBeInTheDocument();
    });

    expect(screen.queryByText("Utilização não exposta por esta fonte")).not.toBeInTheDocument();
    expect(screen.queryByText("CONTROLADORA GRÁFICA")).not.toBeInTheDocument();
  });

  it("renders network throughput and physical storage cards in the performance module", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      switch (command) {
        case "get_current_window_label":
          return Promise.resolve("main") as never;
        case "is_current_window_foreground":
          return Promise.resolve(true) as never;
        case "get_system_overview":
          return Promise.resolve({
            cpuPercent: 12.5,
            memoryUsedGb: 8.2,
            memoryTotalGb: 16.0,
            diskUsedGb: 120.0,
            diskTotalGb: 500.0,
            startupItemCount: 4,
            topProcesses: [],
            sampledAtUnixMs: 1725000000,
            source: "Windows CIM",
          }) as never;
        case "get_performance_details":
          return Promise.resolve({
            gpu: { value: "NVIDIA GeForce RTX 4070" },
          }) as never;
        case "get_performance_devices":
          return Promise.resolve({
            adapters: {
              value: [
                {
                  name: "Ethernet",
                  description: "Realtek PCIe GbE Family Controller",
                  status: "Ativo",
                  hardwareInterface: true,
                  linkSpeedBps: { value: 1_000_000_000 },
                  receivedBps: { value: 1.2 * 1024 ** 2 },
                  sentBps: { value: 256 * 1024 },
                },
              ],
              unavailableReason: null,
            },
            disks: {
              value: [
                {
                  index: 0,
                  model: "Samsung SSD 990 PRO 1TB",
                  mediaType: "SSD",
                  busType: "NVMe",
                  sizeBytes: { value: 1_000_204_886_016 },
                  health: { value: "Saudável" },
                  temperatureCelsius: { value: 38 },
                },
              ],
              unavailableReason: null,
            },
            volumes: [
              {
                path: "C:\\",
                label: "Disco Local (C:)",
                totalBytes: 500 * 1024 ** 3,
                freeBytes: 150 * 1024 ** 3,
              },
            ],
          }) as never;
        default:
          return Promise.resolve() as never;
      }
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Desempenho/ }));

    await waitFor(() => {
      expect(screen.getByText("REDE")).toBeInTheDocument();
      expect(screen.getByText("ARMAZENAMENTO FÍSICO")).toBeInTheDocument();
      expect(screen.getByText("Ethernet")).toBeInTheDocument();
      expect(screen.getByText("Samsung SSD 990 PRO 1TB")).toBeInTheDocument();
      expect(screen.getByText("SSD")).toBeInTheDocument();
      expect(screen.getByText("NVMe")).toBeInTheDocument();
      expect(screen.getByText("Saudável")).toBeInTheDocument();
      expect(screen.getByText("150 GB livres")).toBeInTheDocument();
      expect(screen.getAllByText(/1,2 MB\/s/).length).toBeGreaterThan(0);
      expect(screen.getAllByText(/256 KB\/s/).length).toBeGreaterThan(0);
    });
  });

  it("handles the tray icon and close confirmation modal with exact question", async () => {
    render(<App />);

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await waitFor(() => {
      expect(registeredEventListeners["pulse://close-requested"]).toBeDefined();
    });
    expect(screen.queryByRole("button", { name: "Fechar ou minimizar o aplicativo" })).not.toBeInTheDocument();
    act(() => {
      registeredEventListeners["pulse://close-requested"]({});
    });

    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeInTheDocument();
    expect(
      screen.getByText("Você quer fechar mesmo ou quer ir para o tray icon?"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "CANCELAR" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    act(() => {
      registeredEventListeners["pulse://close-requested"]({});
    });

    await waitFor(() => {
      expect(
        screen.getByText("Você quer fechar mesmo ou quer ir para o tray icon?"),
      ).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: "IR PARA O TRAY ICON" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("minimize_to_tray_and_remember");
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Minimizar para o tray icon" }));
    expect(invoke).toHaveBeenCalledWith("minimize_to_tray");
  });

  it("confirms and closes an application from the Atividade module", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Atividade/ }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Fechar chrome.exe" })).toBeInTheDocument();
    });
    fireEvent.click(screen.getByRole("button", { name: "Fechar chrome.exe" }));
    expect(screen.getByRole("dialog", { name: "Fechar chrome.exe?" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "FECHAR APLICATIVO" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("terminate_process", { id: 1001 });
    });
  });

  it("shows aggregated CPU and GPU usage per process group in Atividade", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("main") as never;
      if (command === "is_current_window_foreground") return Promise.resolve(true) as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 20,
          memoryUsedGb: 6,
          memoryTotalGb: 16,
          diskUsedGb: 100,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [
            {
              name: "chrome.exe",
              displayName: "Google Chrome",
              id: 1001,
              memoryMb: 1024,
              processIds: [1001, 1002],
              processCount: 2,
              cpuPercent: 31.4,
              gpuPercent: 7.2,
            },
            { name: "code.exe", id: 1003, memoryMb: 512, cpuPercent: 4.0, gpuPercent: 0 },
          ],
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Atividade/ }));

    await waitFor(() => {
      expect(screen.getByText("31,4%")).toBeInTheDocument();
    });
    expect(screen.getByText("7,2%")).toBeInTheDocument();
    expect(screen.getByText("4%")).toBeInTheDocument();
    // Zero medido é uso real sem GPU, não indisponibilidade.
    expect(screen.getByText("0%")).toBeInTheDocument();
    expect(screen.getByText("CPU")).toBeInTheDocument();
    expect(screen.getByText("GPU")).toBeInTheDocument();
  });

  it("sorts process groups by RAM, CPU and GPU from the Atividade header", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("main") as never;
      if (command === "is_current_window_foreground") return Promise.resolve(true) as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 20,
          memoryUsedGb: 6,
          memoryTotalGb: 16,
          diskUsedGb: 100,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [
            { name: "chrome.exe", id: 1, memoryMb: 512, cpuPercent: 8, gpuPercent: 2 },
            { name: "code.exe", id: 2, memoryMb: 1024, cpuPercent: 4, gpuPercent: 9 },
            { name: "explorer.exe", id: 3, memoryMb: 300, cpuPercent: 31, gpuPercent: 12 },
          ],
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Atividade/ }));

    await waitFor(() => {
      expect(screen.getByText("31%")).toBeInTheDocument();
    });

    const firstRow = () => document.querySelector(".process-list li")?.textContent ?? "";
    // A coleta entrega o ranking por memória: code (1024 MB) primeiro.
    expect(firstRow()).toContain("code.exe");

    fireEvent.click(screen.getByRole("button", { name: /Ordenar por CPU/ }));
    expect(firstRow()).toContain("explorer.exe");

    fireEvent.click(screen.getByRole("button", { name: /Ordenar por CPU/ }));
    expect(firstRow()).toContain("code.exe");
    expect(screen.getByRole("button", { name: /Ordenar por CPU, crescente/ })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Ordenar por GPU/ }));
    expect(firstRow()).toContain("explorer.exe");
    expect(screen.getByText(/ordenados por GPU/)).toBeInTheDocument();
  });

  it("marks GPU as unavailable when the driver exposes no engine counters", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("main") as never;
      if (command === "is_current_window_foreground") return Promise.resolve(true) as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 9,
          memoryUsedGb: 5,
          memoryTotalGb: 16,
          diskUsedGb: 100,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [
            { name: "explorer.exe", id: 900, memoryMb: 300, cpuPercent: 1.5, gpuPercent: null },
          ],
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Atividade/ }));

    await waitFor(() => {
      expect(screen.getByText("GPU —")).toBeInTheDocument();
    });
    expect(screen.queryByText("GPU")).not.toBeInTheDocument();
  });

  it("explains the wider impact before shutting down vmmemWSL", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Atividade/ }));

    const closeButton = await screen.findByRole("button", { name: "Fechar vmmemWSL" });
    fireEvent.click(closeButton);

    const dialog = screen.getByRole("dialog", { name: "Fechar vmmemWSL?" });
    expect(dialog).toHaveTextContent("todas as distribuições e processos Linux");
    fireEvent.click(screen.getByRole("button", { name: "DESLIGAR WSL" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("terminate_process", { id: 1003 });
    });
  });

  it("filters and sorts recent files with visible dates and sizes", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Armazenamento/ }));
    fireEvent.click(screen.getByRole("button", { name: "ARQUIVOS RECENTES" }));

    await waitFor(() => {
      expect(screen.getByText("relatorio.pdf")).toBeInTheDocument();
      expect(screen.getByText("foto.png")).toBeInTheDocument();
      expect(screen.getByText("backup.zip")).toBeInTheDocument();
    });

    // Exibição visível de tamanho e data
    expect(screen.getByText("200,0 KB")).toBeInTheDocument();
    expect(screen.getByText("1,0 MB")).toBeInTheDocument();
    expect(screen.getByText("50,0 MB")).toBeInTheDocument();

    // Filtro de busca
    const searchInput = screen.getByRole("textbox", { name: "Buscar arquivos" });
    fireEvent.change(searchInput, { target: { value: "relatorio" } });
    expect(screen.getByText("relatorio.pdf")).toBeInTheDocument();
    expect(screen.queryByText("foto.png")).not.toBeInTheDocument();
    expect(screen.queryByText("backup.zip")).not.toBeInTheDocument();

    fireEvent.change(searchInput, { target: { value: "" } });
    const categorySelect = screen.getByRole("combobox", { name: "Filtrar categoria" });
    fireEvent.change(categorySelect, { target: { value: "image" } });
    expect(screen.queryByText("relatorio.pdf")).not.toBeInTheDocument();
    expect(screen.getByText("foto.png")).toBeInTheDocument();

    // Ordenação
    fireEvent.change(categorySelect, { target: { value: "" } });
    const sortSelect = screen.getByRole("combobox", { name: "Ordenar arquivos" });

    // Ordenar por tamanho (maior primeiro) -> backup.zip (50MB) deve ser o primeiro
    fireEvent.change(sortSelect, { target: { value: "size" } });
    let rows = Array.from(document.querySelectorAll(".recent-list .file-row"));
    expect(rows[0]?.querySelector(".file-name")?.textContent).toBe("backup.zip");

    // Ordenar por nome (A-Z) -> backup.zip, foto.png, relatorio.pdf
    fireEvent.change(sortSelect, { target: { value: "name" } });
    rows = Array.from(document.querySelectorAll(".recent-list .file-row"));
    expect(rows[0]?.querySelector(".file-name")?.textContent).toBe("backup.zip");
    expect(rows[1]?.querySelector(".file-name")?.textContent).toBe("foto.png");
    expect(rows[2]?.querySelector(".file-name")?.textContent).toBe("relatorio.pdf");

    // Ordenar por mais antigo -> backup.zip (timestamp mais baixo)
    fireEvent.change(sortSelect, { target: { value: "oldest" } });
    rows = Array.from(document.querySelectorAll(".recent-list .file-row"));
    expect(rows[0]?.querySelector(".file-name")?.textContent).toBe("backup.zip");
  });

  it("allows opening recent file and file location", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Armazenamento/ }));
    fireEvent.click(screen.getByRole("button", { name: "ARQUIVOS RECENTES" }));

    await waitFor(() => {
      expect(screen.getByText("foto.png")).toBeInTheDocument();
    });

    const openButtons = screen.getAllByRole("button", { name: "Abrir" });
    fireEvent.click(openButtons[0]);
    expect(invoke).toHaveBeenCalledWith("open_recent_file", {
      path: "C:\\Users\\lucas\\Pictures\\foto.png",
    });

    const locationButtons = screen.getAllByRole("button", { name: "Localização" });
    fireEvent.click(locationButtons[0]);
    expect(invoke).toHaveBeenCalledWith("open_recent_file_location", {
      path: "C:\\Users\\lucas\\Pictures\\foto.png",
    });
  });

  it("allows managing custom scan folders in Armazenamento (including adding and removing D:\\Images\\Screenshots)", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Armazenamento/ }));
    fireEvent.click(screen.getByRole("button", { name: "ARQUIVOS RECENTES" }));

    await waitFor(() => {
      expect(screen.getByLabelText("Remover pasta D:\\Images\\Screenshots")).toBeVisible();
    });

    // Test adding a folder
    const input = screen.getByLabelText("Caminho da nova pasta para vasculhar");
    fireEvent.change(input, { target: { value: "E:\\Projetos" } });
    fireEvent.click(screen.getByRole("button", { name: "+ ADICIONAR PASTA" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("add_custom_scan_folder", {
        path: "E:\\Projetos",
      });
    });

    // Test removing a custom folder
    const removeBtn = screen.getByLabelText("Remover pasta D:\\Images\\Screenshots");
    fireEvent.click(removeBtn);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("remove_custom_scan_folder", {
        path: "D:\\Images\\Screenshots",
      });
    });
  });

  it("renders ScanFoldersBar standalone component correctly and toggles collapse", () => {
    const onAdd = vi.fn();
    const onRemove = vi.fn();
    const onChange = vi.fn();

    render(
      <ScanFoldersBar
        customFolders={["D:\\Images\\Screenshots"]}
        newFolderPath=""
        loading={false}
        onNewFolderPathChange={onChange}
        onAddFolder={onAdd}
        onRemoveFolder={onRemove}
      />,
    );

    expect(screen.getByText("Pastas monitoradas (7)")).toBeInTheDocument();
    expect(screen.getByText("D:\\Images\\Screenshots")).toBeInTheDocument();

    // Toggle collapse
    const toggleBtn = screen.getByRole("button", { name: "OCULTAR PASTAS" });
    fireEvent.click(toggleBtn);
    expect(screen.queryByText("D:\\Images\\Screenshots")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "CONFIGURAR PASTAS" }));
    expect(screen.getByText("D:\\Images\\Screenshots")).toBeInTheDocument();
  });

  it("handles toggling startup item", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Inicialização e segurança/ }));

    await waitFor(() => {
      expect(screen.getByText("OneDrive")).toBeInTheDocument();
    });

    const checkbox = screen.getByRole("checkbox", { name: "Desativar OneDrive" });
    await act(async () => {
      fireEvent.click(checkbox);
    });

    expect(invoke).toHaveBeenCalledWith("set_startup_item_enabled", {
      name: "OneDrive",
      origin: "HKCU\\Run",
      enabled: false,
    });
  });

  it("displays slowest startup items with impact badges and correctly marks disabled items", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "list_startup_items") {
        return Promise.resolve([
          {
            name: "FastUtility",
            command: "fast.exe",
            origin: "HKCU\\Run",
            enabled: true,
            canChange: true,
            impact: "Baixo impacto",
            startupTimeMs: 120,
          },
          {
            name: "HeavyGameClient",
            command: "heavy.exe",
            origin: "HKCU\\Run",
            enabled: true,
            canChange: true,
            impact: "Alto impacto",
            startupTimeMs: 2400,
          },
          {
            name: "DisabledService",
            command: "service.exe",
            origin: "HKLM\\Run",
            enabled: false,
            canChange: true,
            impact: "Médio impacto",
            startupTimeMs: 600,
          },
        ]) as never;
      }
      if (command === "set_startup_item_enabled") {
        return Promise.resolve({
          name: "DisabledService",
          command: "service.exe",
          origin: "HKLM\\Run",
          enabled: true,
          canChange: true,
          impact: "Médio impacto",
          startupTimeMs: 600,
        }) as never;
      }
      return Promise.resolve([]) as never;
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Inicialização e segurança/ }));

    await waitFor(() => {
      expect(screen.getByText("HeavyGameClient")).toBeInTheDocument();
      expect(screen.getByText("DisabledService")).toBeInTheDocument();
      expect(screen.getByText("FastUtility")).toBeInTheDocument();
    });

    // Itens desativados são identificados corretamente
    expect(screen.getByText("Desativado")).toBeInTheDocument();
    const enableCheckbox = screen.getByRole("checkbox", { name: "Ativar DisabledService" });
    expect(enableCheckbox).not.toBeChecked();

    // Verificação de métricas dos mais lentos
    expect(screen.getByText("Alto impacto · 2,4 s")).toBeInTheDocument();
    expect(screen.getByText(/1 aplicativo de alto impacto detectado/)).toBeInTheDocument();

    // Reativação de item desativado
    await act(async () => {
      fireEvent.click(enableCheckbox);
    });
    expect(invoke).toHaveBeenCalledWith("set_startup_item_enabled", {
      name: "DisabledService",
      origin: "HKLM\\Run",
      enabled: true,
    });
  });

  it("uses active items followed by the slowest items as the default startup order", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "list_startup_items") {
        return Promise.resolve([
          { name: "Quick", command: "quick.exe", origin: "HKCU\\Run", enabled: true, canChange: true, impact: "Baixo impacto", startupTimeMs: 120 },
          { name: "Slow", command: "slow.exe", origin: "HKCU\\Run", enabled: true, canChange: true, impact: "Alto impacto", startupTimeMs: 1800 },
          { name: "Off", command: "off.exe", origin: "HKLM\\Run", enabled: false, canChange: true, impact: "Médio impacto", startupTimeMs: 650 },
        ]) as never;
      }
      return Promise.resolve([]) as never;
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Inicialização e segurança/ }));

    await screen.findByText("Slow");
    expect(
      (screen.getByRole("option", { name: "Ativos e mais lentos primeiro" }) as HTMLOptionElement)
        .selected,
    ).toBe(true);
    expect(screen.getAllByText(/Quick|Slow|Off/).map(element => element.textContent)).toEqual([
      "Slow",
      "Quick",
      "Off",
    ]);
  });

  it("enables and displays the Debug diagnostic module", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_debug_mode") return Promise.resolve(true) as never;
      if (command === "get_debug_log") {
        return Promise.resolve([
          {
            sequence: 12,
            timestampUnixMs: 1725000000,
            operation: "system_overview",
            category: "telemetry",
            source: "rust-backend",
            durationMs: 45,
            outcome: "ok",
            severity: "info",
            message: "Concluído",
          },
          {
            sequence: 13,
            timestampUnixMs: 1725000010,
            operation: "subscription_usage_claude",
            category: "telemetry",
            source: "rust-backend",
            durationMs: 120,
            outcome: "ok",
            severity: "info",
            message: "Cota confirmada · Claude Code CLI · /usage",
          },
          {
            sequence: 14,
            timestampUnixMs: 1725000020,
            operation: "subscription_usage_claude",
            category: "telemetry",
            source: "rust-backend",
            durationMs: 110,
            outcome: "ok",
            severity: "info",
            message: "Cota confirmada · Claude Code CLI · /usage",
          },
        ]) as never;
      }
      if (command === "get_debug_audit_info") {
        return Promise.resolve({
          enabled: true,
          sessionStartedUnixMs: 1725000000,
          entryCount: 3,
          retentionLimit: 500,
          applicationVersion: "0.1.0",
          platform: "windows",
          persistence: "Somente memória desta execução",
          dataResidency: "Processado e mantido localmente",
        }) as never;
      }
      return Promise.resolve([]) as never;
    });

    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Debug/ })).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: /Debug/ }));
    await waitFor(() => {
      expect(screen.getByText("Coleta da visão geral")).toBeInTheDocument();
    });
    expect(screen.getByText("system_overview")).toBeInTheDocument();
    expect(screen.getByText("Cota do Claude Code")).toBeInTheDocument();
    expect(screen.getByText("2 ocorrências")).toBeInTheDocument();
    expect(screen.getByText("Somente memória desta execução")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "COPIAR JSON" })).toBeEnabled();
    expect(screen.getByLabelText("Buscar na auditoria")).toBeInTheDocument();
  });

  it("renders when current window is tray-flyout and switches to main on action", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("tray-flyout") as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 10,
          memoryUsedGb: 4,
          memoryTotalGb: 16,
          diskUsedGb: 100,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [{ name: "explorer.exe", id: 200, memoryMb: 300 }],
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary", { name: "Painel rápido do tray" })).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: "Abrir interface completa" }));
    expect(invoke).toHaveBeenCalledWith("show_main_window");
  });

  it("renders tray-flyout with dollar balance, audio selectors and top processes", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("tray-flyout") as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 15,
          memoryUsedGb: 8,
          memoryTotalGb: 16,
          diskUsedGb: 120,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [
            { name: "code.exe", displayName: "Visual Studio Code", id: 101, memoryMb: 1200, publisher: "Microsoft" },
            { name: "chrome.exe", displayName: "Google Chrome", id: 102, memoryMb: 800, publisher: "Google" },
          ],
        }) as never;
      }
      if (command === "get_audio_devices") {
        return Promise.resolve({
          input: [{ id: "mic-1", name: "Microfone Realtek", isDefault: true }],
          output: [{ id: "speaker-1", name: "Alto-falantes Realtek", isDefault: true }],
          inputDefaultId: "mic-1",
          outputDefaultId: "speaker-1",
          unavailableReason: null,
        }) as never;
      }
      if (command === "get_subscription_usage") {
        return Promise.resolve({
          scanStatus: "ready",
          durationMs: 120,
          privacyNote: "Privado",
          sampledAtUnixMs: Date.now(),
          providers: [
            {
              id: "openrouter",
              provider: "OpenRouter",
              subtitle: "Saldo em Dólares",
              status: "available",
              windows: [],
              creditsRemaining: 0.3026,
              source: "openrouter.ai",
            },
          ],
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary", { name: "Painel rápido do tray" })).toBeInTheDocument();
    });

    // Confirma que o header exibe o saldo com cifrão ($)
    await waitFor(() => {
      expect(screen.getByLabelText("Saldo OpenRouter disponível em dólares (USD)")).toBeInTheDocument();
      expect(screen.getByLabelText("Saldo OpenRouter disponível em dólares (USD)")).toHaveTextContent("$");
    });

    // Confirma que os processos principais aparecem legíveis
    expect(screen.getByText("Visual Studio Code")).toBeInTheDocument();
    expect(screen.getByText("Google Chrome")).toBeInTheDocument();

    // Confirma que o card de cota do OpenRouter exibe SALDO e o valor em dólares
    expect(screen.getByText("SALDO")).toBeInTheDocument();
    expect(screen.getAllByText(/\$ 0,30/).length).toBeGreaterThanOrEqual(2);
  });

  it("renders tray-flyout with two quota progress bars and provider icons for multi-window providers", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("tray-flyout") as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 12,
          memoryUsedGb: 6,
          memoryTotalGb: 16,
          diskUsedGb: 100,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [],
        }) as never;
      }
      if (command === "get_subscription_usage") {
        return Promise.resolve({
          scanStatus: "ready",
          durationMs: 80,
          privacyNote: "Privado",
          sampledAtUnixMs: Date.now(),
          providers: [
            {
              id: "claude",
              provider: "Claude",
              plan: "Pro",
              status: "available",
              windows: [
                { id: "session", label: "Sessão atual (5h)", remainingPercent: 80, usedPercent: 20 },
                { id: "weekly", label: "Cota semanal (7d)", remainingPercent: 95, usedPercent: 5 },
              ],
              creditsRemaining: null,
              source: "claude.ai",
            },
          ],
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary", { name: "Painel rápido do tray" })).toBeInTheDocument();
    });

    // Provedor Claude com ícone e plano PRO em linha
    await waitFor(() => {
      expect(screen.getByText("Claude")).toBeInTheDocument();
      expect(screen.getByText("PRO")).toBeInTheDocument();
    });

    // Confirma presença do SVG do ícone oficial do Claude
    const claudeIcon = document.querySelector(".provider-icon-claude");
    expect(claudeIcon).not.toBeNull();

    // Confirma que as 2 barras aparecem com os rótulos compactos 5h e 7d
    expect(screen.getByText("5h")).toBeInTheDocument();
    expect(screen.getByText("7d")).toBeInTheDocument();
    expect(screen.getByText("80%")).toBeInTheDocument();
    expect(screen.getByText("95%")).toBeInTheDocument();
  });

  it("keeps antigravity five-hour and weekly windows labeled by duration in the tray", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_current_window_label") return Promise.resolve("tray-flyout") as never;
      if (command === "get_system_overview") {
        return Promise.resolve({
          cpuPercent: 12,
          memoryUsedGb: 6,
          memoryTotalGb: 16,
          diskUsedGb: 100,
          diskTotalGb: 500,
          startupItemCount: 1,
          topProcesses: [],
        }) as never;
      }
      if (command === "get_subscription_usage") {
        return Promise.resolve({
          scanStatus: "ready",
          durationMs: 80,
          privacyNote: "Privado",
          sampledAtUnixMs: Date.now(),
          providers: [
            {
              id: "antigravity",
              provider: "Antigravity",
              plan: null,
              status: "available",
              windows: [
                { id: "agy-0", label: "Gemini Models · Semanal", remainingPercent: 85, usedPercent: 15, windowMinutes: 10080 },
                { id: "agy-1", label: "Gemini Models · 5 horas", remainingPercent: 100, usedPercent: 0, windowMinutes: 300 },
                { id: "agy-2", label: "Claude and GPT models · Semanal", remainingPercent: 66, usedPercent: 34, windowMinutes: 10080 },
                { id: "agy-3", label: "Claude and GPT models · 5 horas", remainingPercent: 100, usedPercent: 0, windowMinutes: 300 },
              ],
              creditsRemaining: null,
              source: "Antigravity CLI · /usage",
            },
          ],
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary", { name: "Painel rápido do tray" })).toBeInTheDocument();
    });

    await waitFor(() => {
      const labels = [...document.querySelectorAll(".flyout-quota-window-label")].map(node => node.textContent);
      expect(labels).toEqual(["5h", "7d"]);
    });
    expect(screen.getByTitle("Gemini Models · 5 horas")).toBeInTheDocument();
    expect(screen.getByTitle("Claude and GPT models · Semanal")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
    expect(screen.getByText("66%")).toBeInTheDocument();
    expect(screen.queryByText("Gemi")).not.toBeInTheDocument();
  });

  it("renders every primary module with valid native payloads", async () => {
    render(<App />);
    for (const [buttonName, heading] of [
      [/Atividade/, "Grupos de aplicativos em execução"],
      [/Desempenho/, "GPU"],
      [/Armazenamento/, "ANÁLISE DE DISCO (TREE SIZE)"],
      [/Inicialização e segurança/, "Itens de Inicialização"],
    ] as const) {
      fireEvent.click(screen.getByRole("button", { name: buttonName }));
      await waitFor(() => expect(screen.getByText(heading)).toBeInTheDocument());
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    }
  });

  it("CrashBoundary catches runtime errors and allows recovering to Visão geral", () => {
    const onRecover = vi.fn();
    const ThrowingComponent = () => {
      throw new Error("Erro de renderização forçado");
    };

    render(
      <CrashBoundary onRecover={onRecover}>
        <ThrowingComponent />
      </CrashBoundary>,
    );

    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("Erro de renderização forçado")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "VOLTAR À VISÃO GERAL" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "VOLTAR À VISÃO GERAL" }));
    expect(onRecover).toHaveBeenCalled();
  });

  describe("DiskAnalyzer (Tree Size View)", () => {
    it("extracts top heavy files and file extensions correctly", () => {
      expect(getFileExtension("C:\\Games\\Cyberpunk.iso")).toBe("ISO");
      expect(getFileExtension("data.tar.gz")).toBe("GZ");
      expect(getFileExtension("noextension")).toBe("");

      const mockRoot = {
        id: "root",
        name: "root",
        path: "C:\\",
        sizeBytes: 1000,
        fileCount: 3,
        dirCount: 1,
        isDir: true,
        children: [
          {
            id: "f1",
            name: "small.txt",
            path: "C:\\small.txt",
            sizeBytes: 100,
            fileCount: 1,
            dirCount: 0,
            isDir: false,
            children: [],
          },
          {
            id: "dir1",
            name: "dir1",
            path: "C:\\dir1",
            sizeBytes: 900,
            fileCount: 2,
            dirCount: 1,
            isDir: true,
            children: [
              {
                id: "f2",
                name: "heavy.bin",
                path: "C:\\dir1\\heavy.bin",
                sizeBytes: 800,
                fileCount: 1,
                dirCount: 0,
                isDir: false,
                children: [],
              },
            ],
          },
        ],
      };

      const topFiles = extractTopFiles(mockRoot, 10);
      expect(topFiles.length).toBe(2);
      expect(topFiles[0].name).toBe("heavy.bin");
      expect(topFiles[0].sizeBytes).toBe(800);
      expect(topFiles[1].name).toBe("small.txt");
    });

    it("runs disk scan and allows navigating through tree window with folder/file discrimination", async () => {
      render(<App />);
      fireEvent.click(screen.getByRole("button", { name: /Armazenamento/ }));

      // Verifica presença das sub-abas do módulo Armazenamento
      expect(screen.getByText("ANÁLISE DE DISCO (TREE SIZE)")).toBeInTheDocument();
      expect(screen.getByText("ARQUIVOS RECENTES")).toBeInTheDocument();
      expect(screen.getByText("LIMPEZA DE DISCO")).toBeInTheDocument();
      expect(screen.queryByText("PASTAS DO ESCOPO:")).not.toBeInTheDocument();
      expect(screen.queryByRole("textbox", { name: "Caminho personalizado para varredura" })).not.toBeInTheDocument();
      expect(screen.getAllByRole("combobox", { name: "Selecionar unidade de disco" })).toHaveLength(1);

      // Botão de escanear disco
      const scanBtn = screen.getByRole("button", { name: /ESCANEAR DISCO/ });
      expect(scanBtn).toBeInTheDocument();

      fireEvent.click(scanBtn);
      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("start_disk_scan", expect.objectContaining({ path: "C:\\" }));
      });

      // Aguarda os dados do mock serem renderizados na Tree Window
      await waitFor(() => {
        expect(screen.getByText("Program Files")).toBeInTheDocument();
        expect(screen.getByText("pagefile.sys")).toBeInTheDocument();
      });

      // Distinção explícita entre pasta e arquivo
      const folderBadges = screen.getAllByText("PASTA");
      const fileBadges = screen.getAllByText("ARQUIVO");
      expect(folderBadges.length).toBeGreaterThan(0);
      expect(fileBadges.length).toBeGreaterThan(0);

      // Filtro para Apenas Pastas
      const onlyFoldersBtn = screen.getByRole("button", { name: /APENAS PASTAS/ });
      fireEvent.click(onlyFoldersBtn);
      expect(screen.getByText("Program Files")).toBeInTheDocument();
      expect(screen.queryByText("pagefile.sys")).not.toBeInTheDocument();

      // Filtro para Maiores Arquivos (Top Heavy Files)
      const onlyFilesBtn = screen.getByRole("button", { name: /MAIORES ARQUIVOS/ });
      fireEvent.click(onlyFilesBtn);
      await waitFor(() => {
        expect(screen.getByText("pagefile.sys")).toBeInTheDocument();
        expect(screen.queryByText("Program Files")).not.toBeInTheDocument();
      });

      // Retorna para todos os itens
      fireEvent.click(screen.getByRole("button", { name: "TODOS OS ITENS" }));
      expect(screen.getByText("Program Files")).toBeInTheDocument();

      // Navegação: focar em uma pasta na árvore
      const focusButtons = screen.getAllByRole("button", { name: "Focar" });
      fireEvent.click(focusButtons[0]);

      // Verifica se o breadcrumb de localização foi atualizado e exibe botão de subir nível
      await waitFor(() => {
        expect(screen.getByRole("button", { name: "⬅ SUBIR NÍVEL" })).toBeInTheDocument();
      });

      // Volta à raiz
      fireEvent.click(screen.getByRole("button", { name: "VOLTAR À RAIZ" }));
      expect(screen.queryByRole("button", { name: "⬅ SUBIR NÍVEL" })).not.toBeInTheDocument();

      // Ações: Explorar item no Windows Explorer
      const exploreButtons = screen.getAllByRole("button", { name: "Explorar" });
      fireEvent.click(exploreButtons[0]);
      expect(invoke).toHaveBeenCalledWith("open_disk_item", expect.objectContaining({ reveal: true }));
    });
  });

  describe("ToastNotification component", () => {
    it("renders loading toast with spinner and dismisses", () => {
      const onDismiss = vi.fn();
      const { rerender } = render(
        <ToastNotification
          toast={{ type: "loading", message: "Buscando arquivos recentes…" }}
          onDismiss={onDismiss}
        />,
      );

      expect(screen.getByText("BUSCANDO")).toBeInTheDocument();
      expect(screen.getByText("Buscando arquivos recentes…")).toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Fechar notificação" }));
      expect(onDismiss).toHaveBeenCalledTimes(1);

      rerender(<ToastNotification toast={null} onDismiss={onDismiss} />);
      expect(screen.queryByText("BUSCANDO")).not.toBeInTheDocument();
    });

    it("renders success and error toasts correctly", () => {
      const onDismiss = vi.fn();
      const { rerender } = render(
        <ToastNotification
          toast={{ type: "success", message: "Arquivos carregados com sucesso" }}
          onDismiss={onDismiss}
        />,
      );

      expect(screen.getByText("CONCLUÍDO")).toBeInTheDocument();
      expect(screen.getByText("Arquivos carregados com sucesso")).toBeInTheDocument();

      rerender(
        <ToastNotification
          toast={{ type: "error", message: "Erro de varredura" }}
          onDismiss={onDismiss}
        />,
      );
      expect(screen.getByText("ATENÇÃO")).toBeInTheDocument();
      expect(screen.getByText("Erro de varredura")).toBeInTheDocument();
    });
  });

  describe("FileList skeleton loading", () => {
    it("displays skeleton lines when busy is true", () => {
      render(
        <FileList
          files={[]}
          busy={true}
          fileQuery=""
          fileCategory=""
          fileSort="recent"
          onQueryChange={vi.fn()}
          onCategoryChange={vi.fn()}
          onSortChange={vi.fn()}
          onRefresh={vi.fn()}
          onOpen={vi.fn()}
        />,
      );

      expect(screen.getByLabelText("Carregando arquivos recentes…")).toBeInTheDocument();
      expect(screen.queryByText("Nenhum arquivo encontrado.")).not.toBeInTheDocument();
    });
  });
});
