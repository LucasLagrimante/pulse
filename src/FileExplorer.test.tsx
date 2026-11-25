import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  FileExplorer,
  formatExplorerDate,
  formatExplorerSize,
  sortExplorerEntries,
  type DirectoryListing,
  type ExplorerEntry,
  type FilePreview,
  type PinnedFolder,
  type QuickAccessData,
} from "./FileExplorer";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

function makeEntry(overrides: Partial<ExplorerEntry> = {}): ExplorerEntry {
  return {
    name: "documento.txt",
    path: "C:\\Users\\dev\\documento.txt",
    isDir: false,
    isSymlink: false,
    isHidden: false,
    sizeBytes: 2048,
    modifiedUnixMs: 1_760_000_000_000,
    extension: "txt",
    readonly: false,
    ...overrides,
  };
}

function makeQuickAccess(pins: PinnedFolder[] = []): QuickAccessData {
  return {
    defaultPath: "C:\\Users\\dev",
    userFolders: [
      { id: "home", name: "Início", path: "C:\\Users\\dev" },
      { id: "downloads", name: "Downloads", path: "C:\\Users\\dev\\Downloads" },
    ],
    pins,
    drives: [
      {
        path: "C:\\",
        label: "Windows (C:)",
        kind: "fixed",
        totalBytes: 500 * 1024 * 1024 * 1024,
        freeBytes: 120 * 1024 * 1024 * 1024,
      },
      {
        path: "E:\\",
        label: "Leitor Óptico (E:)",
        kind: "removable",
        totalBytes: null,
        freeBytes: null,
      },
    ],
  };
}

function makeListings(): Record<string, DirectoryListing> {
  return {
    "C:\\Users\\dev": {
      path: "C:\\Users\\dev",
      parentPath: "C:\\Users",
      breadcrumbs: [
        { name: "C:", path: "C:\\" },
        { name: "Users", path: "C:\\Users" },
        { name: "dev", path: "C:\\Users\\dev" },
      ],
      entries: [
        makeEntry({
          name: "Projetos",
          path: "C:\\Users\\dev\\Projetos",
          isDir: true,
          sizeBytes: null,
          extension: null,
        }),
        makeEntry({
          name: "Downloads",
          path: "C:\\Users\\dev\\Downloads",
          isDir: true,
          sizeBytes: null,
          extension: null,
        }),
        makeEntry({
          name: "notas.md",
          path: "C:\\Users\\dev\\notas.md",
          isDir: false,
          sizeBytes: 1536,
          extension: "md",
        }),
      ],
      truncated: false,
      totalSeen: 3,
      dirCount: 2,
      fileCount: 1,
    },
    "C:\\Users\\dev\\Projetos": {
      path: "C:\\Users\\dev\\Projetos",
      parentPath: "C:\\Users\\dev",
      breadcrumbs: [
        { name: "C:", path: "C:\\" },
        { name: "Users", path: "C:\\Users" },
        { name: "dev", path: "C:\\Users\\dev" },
        { name: "Projetos", path: "C:\\Users\\dev\\Projetos" },
      ],
      entries: [
        makeEntry({
          name: "main.rs",
          path: "C:\\Users\\dev\\Projetos\\main.rs",
          isDir: false,
          sizeBytes: 4096,
          extension: "rs",
        }),
      ],
      truncated: false,
      totalSeen: 1,
      dirCount: 0,
      fileCount: 1,
    },
    "C:\\Users\\dev\\Downloads": {
      path: "C:\\Users\\dev\\Downloads",
      parentPath: "C:\\Users\\dev",
      breadcrumbs: [
        { name: "C:", path: "C:\\" },
        { name: "Users", path: "C:\\Users" },
        { name: "dev", path: "C:\\Users\\dev" },
        { name: "Downloads", path: "C:\\Users\\dev\\Downloads" },
      ],
      entries: [],
      truncated: false,
      totalSeen: 0,
      dirCount: 0,
      fileCount: 0,
    },
    "C:\\Users": {
      path: "C:\\Users",
      parentPath: "C:\\",
      breadcrumbs: [
        { name: "C:", path: "C:\\" },
        { name: "Users", path: "C:\\Users" },
      ],
      entries: [
        makeEntry({
          name: "dev",
          path: "C:\\Users\\dev",
          isDir: true,
          sizeBytes: null,
          extension: null,
        }),
      ],
      truncated: false,
      totalSeen: 1,
      dirCount: 1,
      fileCount: 0,
    },
  };
}

describe("FileExplorer helpers", () => {
  it("formats sizes and dates defensively without fake zeros", () => {
    expect(formatExplorerSize(null)).toBe("Indisponível");
    expect(formatExplorerSize(undefined)).toBe("Indisponível");
    expect(formatExplorerSize(Number.NaN)).toBe("Indisponível");
    expect(formatExplorerSize(-5)).toBe("Indisponível");
    expect(formatExplorerSize(512)).toBe("512 B");
    expect(formatExplorerSize(2048)).toBe("2,0 KB");

    expect(formatExplorerDate(null)).toBe("Data indisponível");
    expect(formatExplorerDate(undefined)).toBe("Data indisponível");
    expect(formatExplorerDate(0)).toBe("Data indisponível");
    expect(formatExplorerDate(Number.NaN)).toBe("Data indisponível");
  });

  it("keeps directories before files regardless of sort column", () => {
    const entries = [
      makeEntry({ name: "alpha.txt", isDir: false, sizeBytes: 10 }),
      makeEntry({ name: "zeta", isDir: true, sizeBytes: null }),
      makeEntry({ name: "beta", isDir: true, sizeBytes: null }),
    ];
    const sorted = sortExplorerEntries(entries, "name", "desc");
    expect(sorted.map(e => e.name)).toEqual(["zeta", "beta", "alpha.txt"]);
  });
});

describe("FileExplorer component", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("loads quick access, renders drives honestly and supports multi-tab navigation with history", async () => {
    const listings = makeListings();
    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "explorer_get_quick_access") {
        return Promise.resolve(makeQuickAccess()) as never;
      }
      if (command === "explorer_list_directory") {
        const { path, showHidden } = args as { path: string; showHidden: boolean };
        const base = listings[path] ?? listings["C:\\Users\\dev"];
        if (showHidden) {
          return Promise.resolve({
            ...base,
            entries: [
              ...base.entries,
              makeEntry({
                name: ".gitignore",
                path: `${base.path}\\.gitignore`,
                isHidden: true,
                sizeBytes: 128,
                extension: "gitignore",
              }),
            ],
          }) as never;
        }
        return Promise.resolve(base) as never;
      }
      if (command === "explorer_preview_file") {
        const { path } = args as { path: string };
        const preview: FilePreview = {
          path,
          name: "notas.md",
          isDir: false,
          isSymlink: false,
          isHidden: false,
          readonly: false,
          sizeBytes: 1536,
          modifiedUnixMs: 1_760_000_000_000,
          createdUnixMs: 1_759_000_000_000,
          extension: "md",
          kind: "text",
          textSnippet: "# Plano do Pulse\nConteudo de teste",
          textTruncated: false,
          imageDataUrl: null,
          note: null,
        };
        return Promise.resolve(preview) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<FileExplorer />);

    await waitFor(() => {
      expect(screen.getByText("notas.md")).toBeInTheDocument();
    });

    // Drive sem capacidade mostra aviso honesto em vez de 0 B
    expect(screen.getByText("Windows (C:)")).toBeInTheDocument();
    expect(screen.getByText("Leitor Óptico (E:)")).toBeInTheDocument();
    expect(screen.getByText("Capacidade indisponível")).toBeInTheDocument();

    // Seleciona notas.md e confere preview de texto
    fireEvent.click(screen.getByText("notas.md"));
    await waitFor(() => {
      expect(screen.getByText(/# Plano do Pulse/)).toBeInTheDocument();
    });

    // Duplo clique em Projetos navega dentro da aba 1
    fireEvent.doubleClick(screen.getByText("Projetos"));
    await waitFor(() => {
      expect(screen.getByText("main.rs")).toBeInTheDocument();
    });

    // Abre nova aba: aba 2 tem histórico próprio independente
    fireEvent.click(screen.getByRole("button", { name: "Nova aba" }));
    await waitFor(() => {
      expect(screen.getAllByRole("tab")).toHaveLength(2);
    });

    // Na aba 1, botão Voltar retorna para C:\Users\dev
    const tabs = screen.getAllByRole("tab");
    fireEvent.click(tabs[0]);
    const backBtn = screen.getByRole("button", { name: "Voltar" });
    expect(backBtn).not.toBeDisabled();
    fireEvent.click(backBtn);

    await waitFor(() => {
      expect(screen.getByText("notas.md")).toBeInTheDocument();
    });

    // Toggle "Ocultos" recarrega listagem incluindo arquivos ocultos
    fireEvent.click(screen.getByRole("button", { name: /Ocultos/i }));
    await waitFor(() => {
      expect(screen.getByText(".gitignore")).toBeInTheDocument();
    });
  });

  it("supports instant filtering, recursive subfolder search, view toggle and path editing", async () => {
    const listings = makeListings();
    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "explorer_get_quick_access") {
        return Promise.resolve(makeQuickAccess()) as never;
      }
      if (command === "explorer_list_directory") {
        const { path } = args as { path: string };
        return Promise.resolve(listings[path] ?? listings["C:\\Users\\dev"]) as never;
      }
      if (command === "explorer_search_in_dir") {
        return Promise.resolve([
          makeEntry({
            name: "main.rs",
            path: "C:\\Users\\dev\\Projetos\\main.rs",
            sizeBytes: 4096,
            extension: "rs",
          }),
        ]) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<FileExplorer />);

    await waitFor(() => {
      expect(screen.getByText("notas.md")).toBeInTheDocument();
    });

    // Filtro instantâneo
    const searchInput = screen.getByLabelText("Filtrar itens ou pesquisar");
    fireEvent.change(searchInput, { target: { value: "main" } });
    expect(screen.queryByText("notas.md")).not.toBeInTheDocument();

    // Busca nas subpastas
    fireEvent.click(screen.getByRole("button", { name: /Buscar nas subpastas/i }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("explorer_search_in_dir", {
        root: "C:\\Users\\dev",
        query: "main",
        maxResults: 200,
      });
    });
    expect(screen.getByText("main.rs")).toBeInTheDocument();

    // Volta à pasta e alterna para grade
    fireEvent.click(screen.getByRole("button", { name: "Voltar à pasta" }));
    fireEvent.click(screen.getByRole("button", { name: "Visualização em grade" }));
    expect(screen.getByRole("list")).toBeInTheDocument();

    // Edição direta do caminho via input
    fireEvent.click(screen.getByRole("button", { name: "Editar caminho" }));
    const pathInput = screen.getByLabelText("Caminho da pasta");
    fireEvent.change(pathInput, { target: { value: "C:\\Users" } });
    fireEvent.keyDown(pathInput, { key: "Enter" });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("explorer_list_directory", {
        path: "C:\\Users",
        showHidden: false,
      });
    });
  });

  it("creates folder, renames safely, pins folder, opens terminal and moves to trash via confirmation", async () => {
    let entries: ExplorerEntry[] = [
      makeEntry({
        name: "Projetos",
        path: "C:\\Users\\dev\\Projetos",
        isDir: true,
        sizeBytes: null,
        extension: null,
      }),
      makeEntry({
        name: "rascunho.txt",
        path: "C:\\Users\\dev\\rascunho.txt",
        sizeBytes: 300,
        extension: "txt",
      }),
    ];
    let pins: PinnedFolder[] = [];

    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "explorer_get_quick_access") {
        return Promise.resolve(makeQuickAccess(pins)) as never;
      }
      if (command === "explorer_list_directory") {
        return Promise.resolve({
          path: "C:\\Users\\dev",
          parentPath: "C:\\Users",
          breadcrumbs: [
            { name: "C:", path: "C:\\" },
            { name: "Users", path: "C:\\Users" },
            { name: "dev", path: "C:\\Users\\dev" },
          ],
          entries,
          truncated: false,
          totalSeen: entries.length,
          dirCount: entries.filter(e => e.isDir).length,
          fileCount: entries.filter(e => !e.isDir).length,
        }) as never;
      }
      if (command === "explorer_create_folder") {
        const { name } = args as { parentPath: string; name: string };
        const created = makeEntry({
          name,
          path: `C:\\Users\\dev\\${name}`,
          isDir: true,
          sizeBytes: null,
          extension: null,
        });
        entries = [...entries, created];
        return Promise.resolve(created) as never;
      }
      if (command === "explorer_rename_entry") {
        const { path, newName } = args as { path: string; newName: string };
        const renamed = makeEntry({
          name: newName,
          path: `C:\\Users\\dev\\${newName}`,
          sizeBytes: 300,
          extension: "txt",
        });
        entries = entries.map(e => (e.path === path ? renamed : e));
        return Promise.resolve(renamed) as never;
      }
      if (command === "explorer_trash_entry") {
        const { path } = args as { path: string };
        entries = entries.filter(e => e.path !== path);
        return Promise.resolve(null) as never;
      }
      if (command === "explorer_toggle_pin") {
        const { path } = args as { path: string };
        if (pins.some(p => p.path === path)) {
          pins = pins.filter(p => p.path !== path);
        } else {
          pins = [...pins, { name: "Projetos", path, pinnedAtUnixMs: 1_760_000_000_000 }];
        }
        return Promise.resolve(pins) as never;
      }
      if (command === "explorer_open_in_terminal") {
        return Promise.resolve(null) as never;
      }
      if (command === "explorer_preview_file") {
        const { path } = args as { path: string };
        const name = path.split(/[\\/]/).pop() ?? path;
        const isDir = !name.includes(".");
        return Promise.resolve({
          path,
          name,
          isDir,
          isSymlink: false,
          isHidden: false,
          readonly: false,
          sizeBytes: isDir ? null : 300,
          modifiedUnixMs: 1_760_000_000_000,
          createdUnixMs: 1_760_000_000_000,
          extension: isDir ? null : "txt",
          kind: isDir ? "directory" : "text",
          textSnippet: isDir ? null : "abc",
          textTruncated: false,
          imageDataUrl: null,
          note: null,
        }) as never;
      }
      return Promise.resolve(null) as never;
    });

    render(<FileExplorer />);

    await waitFor(() => {
      expect(screen.getByText("rascunho.txt")).toBeInTheDocument();
    });

    // Criar nova pasta
    fireEvent.click(screen.getByRole("button", { name: /Nova pasta/i }));
    const folderInput = screen.getByLabelText("Nome da nova pasta");
    fireEvent.change(folderInput, { target: { value: "Relatorios" } });
    fireEvent.click(screen.getByRole("button", { name: "Criar pasta" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("explorer_create_folder", {
        parentPath: "C:\\Users\\dev",
        name: "Relatorios",
      });
    });

    // Menu de contexto em Projetos -> Fixar pasta
    fireEvent.contextMenu(within(screen.getByRole("table")).getByText("Projetos"));
    const menu = screen.getByRole("menu");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Fixar pasta" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("explorer_toggle_pin", {
        path: "C:\\Users\\dev\\Projetos",
      });
    });
    expect(screen.getByRole("button", { name: "Desafixar Projetos" })).toBeInTheDocument();

    // Seleciona rascunho.txt, abre no terminal, renomeia e move para Lixeira
    fireEvent.click(within(screen.getByRole("table")).getByText("rascunho.txt"));
    fireEvent.click(screen.getByRole("button", { name: "Terminal" }));
    expect(invoke).toHaveBeenCalledWith("explorer_open_in_terminal", {
      path: "C:\\Users\\dev\\rascunho.txt",
    });

    fireEvent.click(screen.getByRole("button", { name: "Renomear" }));
    const renameInput = screen.getByLabelText("Novo nome");
    fireEvent.change(renameInput, { target: { value: "final.txt" } });
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Renomear" }));

    await waitFor(() => {
      expect(within(screen.getByRole("table")).getByText("final.txt")).toBeInTheDocument();
    });

    // Mover para a Lixeira com confirmação explícita
    fireEvent.click(screen.getByRole("button", { name: "Lixeira" }));
    const trashDialog = screen.getByRole("dialog");
    expect(within(trashDialog).getByText(/não será apagado permanentemente/i)).toBeInTheDocument();
    fireEvent.click(within(trashDialog).getByRole("button", { name: "Mover para Lixeira" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("explorer_trash_entry", {
        path: "C:\\Users\\dev\\final.txt",
      });
    });
    await waitFor(() => {
      expect(within(screen.getByRole("table")).queryByText("final.txt")).not.toBeInTheDocument();
    });
  });
});
