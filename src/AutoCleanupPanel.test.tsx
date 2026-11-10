import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AutoCleanupPanel, describeAutoCleanup, type AutoCleanupStatus } from "./AutoCleanupPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));

const baseStatus: AutoCleanupStatus = {
  enabled: false,
  intervalDays: 7,
  categoryIds: ["user-temp"],
  lastRunAtUnixMs: null,
  lastResult: null,
  nextRunAtUnixMs: null,
  minAgeDays: 7,
  availableCategories: [
    { id: "user-temp", name: "Arquivos temporários do usuário", recommended: true },
    { id: "dev-package-caches", name: "Caches de ferramentas de desenvolvimento", recommended: false },
  ],
};

describe("AutoCleanupPanel", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("describes disabled, pending and completed states without fake numbers", () => {
    expect(describeAutoCleanup(baseStatus)).toMatch(/^Desativada/);
    expect(describeAutoCleanup({ ...baseStatus, enabled: true, nextRunAtUnixMs: 0 }, 1000)).toBe(
      "Ainda não executada · Próxima: assim que o PC ficar ocioso",
    );
    const done = describeAutoCleanup(
      {
        ...baseStatus,
        enabled: true,
        lastRunAtUnixMs: Date.UTC(2026, 10, 1, 12),
        lastResult: { reclaimedBytes: 2 * 1024 ** 3, deletedFiles: 10, deletedDirectories: 1, skippedItems: 0 },
        nextRunAtUnixMs: Date.UTC(2026, 10, 8, 12),
      },
      Date.UTC(2026, 10, 2),
    );
    expect(done).toContain("2,0 GB liberados");
    expect(done).toContain("Próxima:");
  });

  it("persists the toggle and selected categories through the backend", async () => {
    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "get_auto_cleanup") return Promise.resolve(baseStatus) as never;
      if (command === "set_auto_cleanup") {
        return Promise.resolve({ ...baseStatus, ...(args as Partial<AutoCleanupStatus>) }) as never;
      }
      return Promise.resolve() as never;
    });

    render(<AutoCleanupPanel />);
    const toggle = await screen.findByRole("checkbox", { name: "Ativar limpeza automática" });
    expect(toggle).not.toBeChecked();

    fireEvent.click(toggle);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_auto_cleanup", {
        enabled: true,
        intervalDays: 7,
        categoryIds: ["user-temp"],
      }),
    );
    expect(await screen.findByRole("checkbox", { name: "Ativar limpeza automática" })).toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "CONFIGURAR" }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Caches de ferramentas/ }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_auto_cleanup", {
        enabled: true,
        intervalDays: 7,
        categoryIds: ["user-temp", "dev-package-caches"],
      }),
    );
  });

  it("shows the backend validation error instead of silently enabling", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_auto_cleanup") return Promise.resolve({ ...baseStatus, categoryIds: [] }) as never;
      if (command === "set_auto_cleanup") {
        return Promise.reject("Escolha ao menos uma categoria para a limpeza automática.") as never;
      }
      return Promise.resolve() as never;
    });

    render(<AutoCleanupPanel />);
    fireEvent.click(await screen.findByRole("checkbox", { name: "Ativar limpeza automática" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Escolha ao menos uma categoria");
    expect(screen.getByRole("checkbox", { name: "Ativar limpeza automática" })).not.toBeChecked();
  });
});
