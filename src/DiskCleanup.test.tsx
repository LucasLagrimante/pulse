import { fireEvent, render, screen, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DiskCleanup } from "./DiskCleanup";
import { resetPrivilegeCache } from "./PrivilegeIndicator";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));

const category = (id: string, name: string, bytes: number, recommended = true) => ({
  id,
  name,
  description: `${name} descrição`,
  recommended,
  availableBytes: bytes,
  itemCount: 1,
  isPartial: false,
  largestItems: [],
});

describe("DiskCleanup grouped layout", () => {
  beforeEach(() => {
    resetPrivilegeCache();
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_privilege_status") {
        return Promise.resolve({ elevated: false, limitations: [] }) as never;
      }
      if (command === "analyze_disk_cleanup") {
        return Promise.resolve({
          scannedAtUnixMs: Date.now(),
          categories: [
            category("user-temp", "Arquivos temporários do usuário", 2 * 1024 ** 3),
            category("nvidia-shader-cache", "Cache de shaders NVIDIA", 1024 ** 3),
            category("dev-package-caches", "Caches de ferramentas de desenvolvimento", 1024 ** 3, false),
            { ...category("windows-temp", "Temporários do Windows", 0), requiresAdmin: true },
          ],
        }) as never;
      }
      if (command === "get_cleanup_insights") {
        return Promise.resolve([
          {
            id: "vhdx-0",
            title: "Compactar disco virtual · Docker Desktop",
            detail: "Libera o espaço vazio.",
            path: "C:\\Users\\x\\docker_data.vhdx",
            sizeBytes: 45 * 1024 ** 3,
            unavailableReason: null,
            warning: "Nunca apague o .vhdx.",
            command: "wsl --shutdown",
          },
          {
            id: "windows-component-store",
            title: "Componentes do Windows (WinSxS)",
            detail: "DISM oficial.",
            path: null,
            sizeBytes: null,
            unavailableReason: "Não medido: hard links",
            warning: "Use apenas o DISM.",
            command: "DISM /Online",
          },
        ]) as never;
      }
      return Promise.resolve() as never;
    });
  });

  it("groups categories, totals recoverable space and quick-selects", async () => {
    render(<DiskCleanup />);
    fireEvent.click(screen.getByRole("button", { name: "ANALISAR AGORA" }));

    const gpu = await screen.findByRole("region", { name: "Jogos e GPU" });
    // Sem administrador, só a categoria de pasta do sistema ganha o aviso.
    expect(screen.getAllByText("EXIGE ADMINISTRADOR")).toHaveLength(1);
    expect(within(gpu).getByText("Cache de shaders NVIDIA")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Desenvolvimento" })).toBeInTheDocument();
    expect(screen.getByText("4,0 GB")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /ferramentas de desenvolvimento/ })).not.toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "LIMPAR SELEÇÃO" }));
    expect(screen.getByText("0 categorias selecionadas")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "RECOMENDADAS" }));
    expect(screen.getByText("2 categorias selecionadas")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "REVISAR E LIMPAR" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Cache de shaders NVIDIA")).toBeInTheDocument();
  });

  it("shows guided recovery with honest unavailable sizes and never runs commands", async () => {
    render(<DiskCleanup />);
    fireEvent.click(screen.getByRole("button", { name: "ANALISAR AGORA" }));

    const guided = await screen.findByRole("region", { name: "Recuperação guiada" });
    expect(within(guided).getByText("45,0 GB")).toBeInTheDocument();
    expect(within(guided).getByText("Não medido: hard links")).toBeInTheDocument();
    expect(within(guided).getAllByRole("button", { name: "COPIAR COMANDOS" })).toHaveLength(2);
    expect(invoke).not.toHaveBeenCalledWith(expect.stringMatching(/compact|dism/i), expect.anything());
  });
});
