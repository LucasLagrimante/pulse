import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LauncherWindow, type LauncherResponse } from "./LauncherWindow";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

function makeResponse(overrides: Partial<LauncherResponse> = {}): LauncherResponse {
  return {
    query: "",
    category: "all",
    calc: null,
    recent: [],
    items: [],
    indexedAppCount: 42,
    isIndexing: false,
    searchElapsedMs: 2,
    ...overrides,
  };
}

describe("LauncherWindow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders Recentes, quick Pulse access and calculator tip when query is empty", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "launcher_query") {
        return Promise.resolve(
          makeResponse({
            recent: [
              {
                id: "app:vscode",
                kind: "app",
                title: "Visual Studio Code",
                subtitle: "C:\\Program Files\\VS Code\\Code.exe",
                target: "C:\\Program Files\\VS Code\\Code.exe",
                category: "apps",
                badge: "App",
                iconHint: "app",
                actionLabel: "Abrir",
                isRecent: true,
                score: 300,
              },
            ],
            items: [
              {
                id: "pulse:overview",
                kind: "module",
                title: "Visão geral",
                subtitle: "Painel principal de CPU, memória, disco e saúde do sistema",
                target: "Visão geral",
                category: "pulse",
                badge: "Pulse",
                iconHint: "pulse:overview",
                actionLabel: "Abrir módulo",
                isRecent: false,
                score: 100,
              },
            ],
          }),
        ) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<LauncherWindow />);

    await waitFor(() => {
      expect(screen.getByText("Visual Studio Code")).toBeInTheDocument();
    });
    expect(screen.getByText("Recentes")).toBeInTheDocument();
    expect(screen.getByText("Acesso rápido do Pulse")).toBeInTheDocument();
    expect(screen.getByText("Visão geral")).toBeInTheDocument();
    expect(screen.getByText("1280 * 0.85")).toBeInTheDocument();
    expect(screen.getByText("42 apps no índice")).toBeInTheDocument();
  });

  it("shows instant calculator hero card and copies result on Enter", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "launcher_query") {
        const params = args as { query: string };
        if (params.query === "1280 * 0.85") {
          return Promise.resolve(
            makeResponse({
              query: "1280 * 0.85",
              calc: { expression: "1280 * 0.85", formatted: "1088" },
              items: [],
            }),
          ) as never;
        }
        return Promise.resolve(makeResponse()) as never;
      }
      if (command === "launcher_execute") {
        return Promise.resolve({
          ok: true,
          action: "copied",
          message: "Resultado copiado: 1088",
          closeLauncher: false,
        }) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<LauncherWindow />);
    const input = screen.getByLabelText("Buscar no Pulse Runner");
    fireEvent.change(input, { target: { value: "1280 * 0.85" } });

    await waitFor(() => {
      expect(screen.getByText("1088")).toBeInTheDocument();
    });

    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(screen.getByText("Copiado!")).toBeInTheDocument();
    });
    expect(writeText).toHaveBeenCalledWith("1088");
    expect(invoke).toHaveBeenCalledWith("launcher_execute", {
      itemId: "calc:result",
      kind: "calc",
      target: "1088",
      title: "1088",
    });
  });

  it("supports keyboard navigation (ArrowDown, Tab, Enter, Escape)", async () => {
    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "launcher_query") {
        const params = args as { query: string; category: string };
        return Promise.resolve(
          makeResponse({
            query: params.query,
            category: params.category,
            items: [
              {
                id: "pulse:activity",
                kind: "module",
                title: "Atividade",
                subtitle: "Processos em execução",
                target: "Atividade",
                category: "pulse",
                badge: "Pulse",
                iconHint: "pulse:activity",
                actionLabel: "Abrir módulo",
                isRecent: false,
                score: 500,
              },
              {
                id: "sys:terminal",
                kind: "system",
                title: "Terminal",
                subtitle: "Abre uma nova janela de terminal",
                target: "system:terminal",
                category: "system",
                badge: "Sistema",
                iconHint: "system:terminal",
                actionLabel: "Abrir",
                isRecent: false,
                score: 400,
              },
            ],
          }),
        ) as never;
      }
      if (command === "launcher_execute") {
        return Promise.resolve({
          ok: true,
          action: "executed",
          message: "Ação executada: Terminal",
          closeLauncher: true,
        }) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<LauncherWindow />);
    await waitFor(() => {
      expect(screen.getByText("Terminal")).toBeInTheDocument();
    });

    const input = screen.getByLabelText("Buscar no Pulse Runner");

    // Tab muda o chip de filtro de Tudo -> Apps
    fireEvent.keyDown(input, { key: "Tab" });
    await waitFor(() => {
      expect(screen.getByRole("tab", { name: "Apps" })).toHaveAttribute("aria-selected", "true");
    });

    // Seta para baixo seleciona o segundo item (Terminal) e Enter executa
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("launcher_execute", {
        itemId: "sys:terminal",
        kind: "system",
        target: "system:terminal",
        title: "Terminal",
      });
    });

    // Escape chama launcher_hide
    fireEvent.keyDown(input, { key: "Escape" });
    expect(invoke).toHaveBeenCalledWith("launcher_hide");
  });

  it("resets query and focuses input when pulse://launcher-opened event fires", async () => {
    let openedCallback: (() => void) | undefined;
    vi.mocked(listen).mockImplementation(((event: string, cb: () => void) => {
      if (event === "pulse://launcher-opened") {
        openedCallback = cb;
      }
      return Promise.resolve(() => {});
    }) as never);

    vi.mocked(invoke).mockResolvedValue(makeResponse() as never);

    render(<LauncherWindow />);
    const input = screen.getByLabelText("Buscar no Pulse Runner") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "notepad" } });
    expect(input.value).toBe("notepad");

    await waitFor(() => {
      expect(openedCallback).toBeDefined();
    });

    await act(async () => {
      openedCallback?.();
    });

    await waitFor(() => {
      expect(input.value).toBe("");
    });
  });
});
