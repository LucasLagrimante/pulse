import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AiDiskAdvisorModal, type AiDiskAnalysisPreview } from "./AiDiskAdvisorModal";
import type { CachedDiskScan } from "./diskScanStore";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockCachedScan: CachedDiskScan = {
  completedAtUnixMs: 1_725_000_000_000,
  durationMs: 2500,
  isPartial: false,
  itemsScanned: 1540,
  result: {
    id: "C:\\",
    name: "C:\\",
    path: "C:\\",
    sizeBytes: 150_000_000_000,
    fileCount: 1540,
    dirCount: 80,
    isDir: true,
    children: [],
  },
};

const mockPreview: AiDiskAnalysisPreview = {
  provider: "antigravity",
  providerName: "Antigravity (AGY)",
  drivePath: "C:\\",
  command: 'agy "Analise o relatório de armazenamento do Pulse para C:\\"',
  prompt: "Analise o relatório de armazenamento do Pulse para C:\\. Objetivo: quais pastas e arquivos são maiores e onde tem espaço seguro para limpar?",
  contextFilePath: "C:\\Users\\Lucas\\AppData\\Local\\Temp\\pulse-disk-analysis-C.md",
  topDirectories: [
    {
      path: "C:\\Users\\Lucas\\AppData\\Local",
      name: "AppData\\Local",
      sizeBytes: 45_000_000_000,
      sizeFormatted: "45.0 GB",
      isDir: true,
      fileCount: 12000,
    },
    {
      path: "C:\\projects",
      name: "projects",
      sizeBytes: 30_000_000_000,
      sizeFormatted: "30.0 GB",
      isDir: true,
      fileCount: 25000,
    },
  ],
  topFiles: [
    {
      path: "C:\\Users\\Lucas\\Downloads\\ubuntu.iso",
      name: "ubuntu.iso",
      sizeBytes: 4_500_000_000,
      sizeFormatted: "4.50 GB",
      isDir: false,
      fileCount: 1,
    },
  ],
  cleanupCandidates: [
    {
      path: "C:\\Users\\Lucas\\AppData\\Local\\Temp",
      name: "Temp",
      sizeBytes: 8_500_000_000,
      sizeFormatted: "8.50 GB",
      isDir: true,
      fileCount: 3500,
    },
  ],
  totalCandidatesBytes: 8_500_000_000,
  totalCandidatesFormatted: "8.50 GB",
  isAvailable: true,
  environment: "WSL (Ubuntu)",
};

describe("AiDiskAdvisorModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("does not render when isOpen is false", () => {
    render(
      <AiDiskAdvisorModal
        isOpen={false}
        onClose={vi.fn()}
        cachedScan={mockCachedScan}
      />,
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("renders modal header, providers and fetches preview when open", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_ai_disk_analysis_preview") {
        return Promise.resolve(mockPreview) as never;
      }
      return Promise.resolve() as never;
    });

    render(
      <AiDiskAdvisorModal
        isOpen={true}
        onClose={vi.fn()}
        cachedScan={mockCachedScan}
      />,
    );

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(
      screen.getByText("Otimização de Armazenamento e Limpeza de Disco"),
    ).toBeInTheDocument();

    // 3 provedores suportados presentes
    expect(screen.getByRole("radio", { name: /Antigravity/ })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Claude Code/ })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Codex CLI/ })).toBeInTheDocument();

    await waitFor(() => {
      expect(screen.getAllByText("45.0 GB").length).toBeGreaterThanOrEqual(1);
      expect(screen.getAllByText("8.50 GB").length).toBeGreaterThanOrEqual(1);
      expect(screen.getAllByText("WSL (Ubuntu)").length).toBeGreaterThanOrEqual(1);
    });
  });

  it("switches providers and updates command", async () => {
    vi.mocked(invoke).mockImplementation((command: string, args: unknown) => {
      if (command === "get_ai_disk_analysis_preview") {
        const payload = args as { provider: string };
        const cmd = payload.provider === "antigravity" ? "agy" : payload.provider;
        return Promise.resolve({
          ...mockPreview,
          provider: payload.provider,
          providerName: payload.provider === "claude" ? "Claude Code" : "Codex CLI",
          command: `${cmd} "Analise o relatório"`,
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(
      <AiDiskAdvisorModal
        isOpen={true}
        onClose={vi.fn()}
        cachedScan={mockCachedScan}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText((_, el) => el?.tagName.toLowerCase() === "pre" && !!el.textContent?.includes("agy"))).toBeInTheDocument();
    });

    // Clica em Claude Code
    fireEvent.click(screen.getByRole("radio", { name: /Claude Code/ }));

    await waitFor(() => {
      expect(screen.getByText((_, el) => el?.tagName.toLowerCase() === "pre" && !!el.textContent?.includes("claude"))).toBeInTheDocument();
    });
  });

  it("launches the terminal when clicking the primary action button", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_ai_disk_analysis_preview") {
        return Promise.resolve(mockPreview) as never;
      }
      if (command === "launch_ai_disk_terminal") {
        return Promise.resolve({
          success: true,
          provider: "antigravity",
          command: mockPreview.command,
          contextFilePath: mockPreview.contextFilePath,
          terminalType: "Windows Terminal",
          message: "Terminal do Windows aberto com sucesso.",
        }) as never;
      }
      return Promise.resolve() as never;
    });

    render(
      <AiDiskAdvisorModal
        isOpen={true}
        onClose={vi.fn()}
        cachedScan={mockCachedScan}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText("ABRIR NO TERMINAL")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText("ABRIR NO TERMINAL"));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("launch_ai_disk_terminal", expect.anything());
      expect(screen.getByText("✓ Windows Terminal iniciado!")).toBeInTheDocument();
    });
  });

  it("allows editing the custom pre-prompt", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_ai_disk_analysis_preview") {
        return Promise.resolve(mockPreview) as never;
      }
      return Promise.resolve() as never;
    });

    render(
      <AiDiskAdvisorModal
        isOpen={true}
        onClose={vi.fn()}
        cachedScan={mockCachedScan}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText("Editar Prompt")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText("Editar Prompt"));

    const textarea = screen.getByLabelText("Editar pré-prompt de análise");
    expect(textarea).toBeInTheDocument();

    fireEvent.change(textarea, { target: { value: "Meu prompt customizado de teste" } });
    expect(textarea).toHaveValue("Meu prompt customizado de teste");

    fireEvent.click(screen.getByText("Restaurar Padrão"));
    expect(screen.queryByLabelText("Editar pré-prompt de análise")).not.toBeInTheDocument();
  });

  it("calls onClose when clicking close button or backdrop", () => {
    const handleClose = vi.fn();
    render(
      <AiDiskAdvisorModal
        isOpen={true}
        onClose={handleClose}
        cachedScan={mockCachedScan}
      />,
    );

    fireEvent.click(screen.getByLabelText("Fechar diálogo"));
    expect(handleClose).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("dialog"));
    expect(handleClose).toHaveBeenCalledTimes(2);
  });
});
