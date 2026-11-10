import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  UninstallHub,
  canCleanRegistry,
  collectAllFolderPaths,
  descendantsOf,
  existingRegistryKeys,
  filterApps,
  formatBytes,
  canMeasureFully,
  dataSize,
  effectiveSize,
  sizeBadge,
  totalFootprint,
  formatEstimatedSize,
  formatInstallDate,
  monogram,
  nodeCheckState,
  selectedFoldersSize,
  sortApps,
  stageLabel,
  toggleNode,
  type AppFootprint,
  type CleanupReport,
  type FolderNode,
  type InstalledApp,
  type InstalledAppsSnapshot,
  type RegistryKeyRef,
  type UninstallCapabilities,
  type UninstallSession,
  type UninstallTargets,
  type UninstallerOutcome,
} from "./UninstallHub";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

function app(overrides: Partial<InstalledApp> = {}): InstalledApp {
  return {
    id: "HKLM|SOFTWARE\\X\\Uninstall\\Foto|64",
    hive: "HKLM",
    view: "64",
    registryPath: "HKLM\\SOFTWARE\\X\\Uninstall\\Foto",
    displayName: "Foto Studio",
    publisher: "Empresa Acme",
    version: "3.1.2",
    installDate: null,
    installLocation: "C:\\Program Files\\Foto",
    estimatedSizeKb: 51200,
    uninstallString: "C:\\Program Files\\Foto\\unins000.exe",
    isMsi: false,
    ...overrides,
  };
}

function folder(overrides: Partial<FolderNode> = {}): FolderNode {
  return {
    id: "C:\\Program Files\\Foto",
    name: "Foto",
    kind: "folder",
    path: "C:\\Program Files\\Foto",
    sizeBytes: 1024,
    childCount: 1,
    children: null,
    blockedReason: null,
    truncated: null,
    ...overrides,
  };
}

function registryKey(overrides: Partial<RegistryKeyRef> = {}): RegistryKeyRef {
  return {
    id: "HKLM|SOFTWARE\\Foto|64",
    hive: "HKLM",
    view: "64",
    path: "HKLM\\SOFTWARE\\Foto",
    reason: "Chave de software com o nome exato do aplicativo (Foto)",
    confidence: "high",
    exists: true,
    ...overrides,
  };
}

function targets(overrides: Partial<UninstallTargets> = {}): UninstallTargets {
  return {
    appId: app().id,
    displayName: "Foto Studio",
    publisher: "Empresa Acme",
    version: "3.1.2",
    uninstallString: "C:\\Program Files\\Foto\\unins000.exe",
    installLocation: "C:\\Program Files\\Foto",
    isMsi: false,
    registryKeys: [registryKey()],
    folders: [
      folder({
        children: [
          folder({
            id: "C:\\Program Files\\Foto\\cache",
            name: "cache",
            path: "C:\\Program Files\\Foto\\cache",
            sizeBytes: 512,
            childCount: 0,
            blockedReason: null,
          }),
        ],
      }),
    ],
    skippedReason: null,
    ...overrides,
  };
}

function snapshot(overrides: Partial<InstalledAppsSnapshot> = {}): InstalledAppsSnapshot {
  return {
    apps: [app(), app({ id: "HKLM|b|64", displayName: "7-Zip", publisher: "Igor Pavlov", estimatedSizeKb: null, isMsi: false, installLocation: "C:\\Program Files\\7-Zip" })],
    totalCount: 2,
    partialReason: null,
    scannedRoots: 4,
    collectedAtUnixMs: 1,
    ...overrides,
  };
}

function session(overrides: Partial<UninstallSession> = {}): UninstallSession {
  return {
    token: "sessao-1",
    targets: targets(),
    elevated: true,
    restorePointSupported: true,
    restorePointUnavailableReason: null,
    startedAtUnixMs: 1,
    ...overrides,
  };
}

function outcome(overrides: Partial<UninstallerOutcome> = {}): UninstallerOutcome {
  return {
    started: true,
    exitCode: 0,
    timedOut: false,
    entryRemoved: true,
    detectionNote: null,
    messages: ["O desinstalador terminou com sucesso"],
    restorePoint: {
      created: true,
      verified: true,
      sequenceNumber: 42,
      description: "Pulse antes de desinstalar Foto Studio",
      unavailableReason: null,
    },
    targets: targets(),
    finishedAtUnixMs: 2,
    ...overrides,
  };
}

function report(overrides: Partial<CleanupReport> = {}): CleanupReport {
  return {
    registrySelected: 1,
    registryRemoved: 1,
    folderSelected: 0,
    folderRemoved: 0,
    freedBytes: 0,
    items: [
      { id: "HKLM|SOFTWARE\\Foto|64", path: "HKLM\\SOFTWARE\\Foto", removed: true, freedBytes: null, message: "Chave removida" },
    ],
    finishedAtUnixMs: 3,
    ...overrides,
  };
}

const CAPABILITIES: UninstallCapabilities = {
  elevated: true,
  restorePointSupported: true,
  restorePointUnavailableReason: null,
  verificationAvailable: true,
};

function defaultInvoke(cmd: string): unknown {
  if (cmd === "list_installed_apps") return snapshot();
  if (cmd === "get_uninstall_capabilities") return CAPABILITIES;
  if (cmd === "start_uninstall_session") return session();
  if (cmd === "run_app_uninstaller") return outcome();
  if (cmd === "cleanup_uninstall_residue") return report();
  if (cmd === "cancel_uninstall_session") return true;
  return {};
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string) => Promise.resolve(defaultInvoke(cmd)));
});

describe("helpers puros", () => {
  it("formata bytes sem inventar zero", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1536)).toBe("1,5 KB");
    expect(formatBytes(null)).toBe("Indisponível");
    expect(formatBytes(undefined)).toBe("Indisponível");
    expect(formatBytes(Number.NaN)).toBe("Indisponível");
    expect(formatBytes(-5)).toBe("Indisponível");
  });

  it("tamanho estimado ausente não vira 0", () => {
    expect(formatEstimatedSize(51200)).toBe("50,0 MB");
    expect(formatEstimatedSize(null)).toBe("Tamanho indisponível");
    expect(formatEstimatedSize(0)).toBe("Tamanho indisponível");
  });

  it("monograma com uma ou duas palavras", () => {
    expect(monogram(app({ displayName: "Foto Studio" }))).toBe("FS");
    expect(monogram(app({ displayName: "7-Zip" }))).toBe("7-");
    expect(monogram(app({ displayName: "  " }))).toBe("?");
  });

  it("filtra e ordena por nome em pt-BR", () => {
    const apps = sortApps(filterApps(snapshot().apps, "zip"));
    expect(apps.map(item => item.displayName)).toEqual(["7-Zip"]);
    expect(filterApps(snapshot().apps, "EMPRESA")).toHaveLength(1);
    expect(filterApps(snapshot().apps, "   ")).toHaveLength(2);
  });

  it("data de instalação AAAAMMDD vira dd/mm/aaaa e inválida vira —", () => {
    expect(formatInstallDate("20240315")).toBe("15/03/2024");
    expect(formatInstallDate("20241345")).toBe("—");
    expect(formatInstallDate(null)).toBe("—");
  });

  it("tamanho efetivo prefere o medido; parcial nunca fica abaixo da estimativa", () => {
    const item = app({ estimatedSizeKb: 1024 });
    const measured: AppFootprint = { appId: item.id, measuredBytes: 5_000_000, complete: true, fileCount: 3 };
    expect(effectiveSize(item, measured)).toEqual({ bytes: 5_000_000, source: "measured" });
    expect(effectiveSize(item, { ...measured, measuredBytes: 10, complete: false })).toEqual({
      bytes: 1024 * 1024,
      source: "partial",
    });
    expect(effectiveSize(item)).toEqual({ bytes: 1024 * 1024, source: "registry" });
    expect(effectiveSize(app({ estimatedSizeKb: null }))).toBeNull();
  });

  it("ordena por tamanho e por data com desconhecidos sempre no fim", () => {
    const big = app({ id: "big", displayName: "Grande", estimatedSizeKb: 10, installDate: "20200101" });
    const small = app({ id: "small", displayName: "Pequeno", estimatedSizeKb: 900, installDate: "20240101" });
    const unknown = app({ id: "unknown", displayName: "Abc", estimatedSizeKb: null, installDate: null });
    const footprints = new Map<string, AppFootprint>([
      ["big", { appId: "big", measuredBytes: 9_000_000_000, complete: true, fileCount: 1 }],
    ]);
    const names = (list: InstalledApp[]) => list.map(item => item.displayName);
    expect(names(sortApps([small, unknown, big], "size", "desc", footprints))).toEqual(["Grande", "Pequeno", "Abc"]);
    expect(names(sortApps([small, unknown, big], "size", "asc", footprints))).toEqual(["Pequeno", "Grande", "Abc"]);
    expect(names(sortApps([big, unknown, small], "installed", "desc"))).toEqual(["Pequeno", "Grande", "Abc"]);
    expect(names(sortApps([big, unknown, small], "name", "desc"))).toEqual(["Pequeno", "Grande", "Abc"]);
  });

  it("selo diferencia medido, deduzido, parcial e estimado", () => {
    const item = app({ estimatedSizeKb: 1024 });
    const base: AppFootprint = { appId: item.id, measuredBytes: 5_000, complete: true, fileCount: 1 };
    expect(sizeBadge(effectiveSize(item, base)!, base).label).toBe("medido");
    const deduced = { ...base, locationSource: "deduced" as const };
    expect(sizeBadge(effectiveSize(item, deduced)!, deduced).label).toBe("deduzido");
    const partial = { ...base, complete: false };
    expect(sizeBadge(effectiveSize(item, partial)!, partial).label).toBe("parcial");
    expect(sizeBadge(effectiveSize(item)!).label).toBe("estimado");
  });

  it("dados contam à parte e entram no total sem inventar zero", () => {
    const item = app({ estimatedSizeKb: null });
    const onlyData: AppFootprint = { appId: item.id, measuredBytes: null, complete: false, fileCount: 0, dataBytes: 700, dataComplete: true };
    expect(dataSize(onlyData)).toEqual({ bytes: 700, partial: false });
    expect(totalFootprint(item, onlyData)).toBe(700);
    expect(totalFootprint(item)).toBeNull();
    expect(dataSize({ ...onlyData, dataBytes: null })).toBeNull();
    expect(totalFootprint(app({ estimatedSizeKb: 1 }), { ...onlyData, dataBytes: 76 })).toBe(1024 + 76);
  });

  it("oferece medição completa só quando algo ficou limitado", () => {
    const base: AppFootprint = { appId: "x", measuredBytes: 10, complete: true, fileCount: 1 };
    expect(canMeasureFully(base)).toBe(false);
    expect(canMeasureFully({ ...base, complete: false })).toBe(true);
    expect(canMeasureFully({ ...base, dataBytes: 5, dataComplete: false })).toBe(true);
    expect(canMeasureFully({ ...base, complete: false, fullScan: true })).toBe(false);
    expect(canMeasureFully(undefined)).toBe(false);
  });

  it("descendentes incluem o próprio nó", () => {
    const tree = targets().folders[0];
    expect(descendantsOf(tree)).toEqual([
      "C:\\Program Files\\Foto",
      "C:\\Program Files\\Foto\\cache",
    ]);
  });

  it("marcar um pai marca a subárvore e desmarcar remove tudo", () => {
    const tree = targets().folders[0];
    const marked = toggleNode(new Set(), tree);
    expect(marked.size).toBe(2);
    expect(nodeCheckState(tree, marked)).toBe(true);
    const cleared = toggleNode(marked, tree);
    expect(cleared.size).toBe(0);
  });

  it("estado parcial do checkbox vem como null", () => {
    const tree = targets().folders[0];
    const partial = new Set(["C:\\Program Files\\Foto\\cache"]);
    expect(nodeCheckState(tree, partial)).toBe(null);
    expect(nodeCheckState(tree, new Set())).toBe(false);
  });

  it("tamanho selecionado não conta duas vezes", () => {
    const tree = targets().folders[0];
    expect(selectedFoldersSize([tree], new Set())).toBe(0);
    expect(selectedFoldersSize([tree], toggleNode(new Set(), tree))).toBe(1024);
    expect(selectedFoldersSize([tree], new Set(["C:\\Program Files\\Foto\\cache"]))).toBe(512);
  });

  it("marcar todos percorre todas as pastas", () => {
    expect(collectAllFolderPaths(targets().folders)).toHaveLength(2);
  });

  it("chaves inexistentes não entram na limpeza", () => {
    const keys = [registryKey(), registryKey({ id: "x", exists: false })];
    expect(existingRegistryKeys(keys)).toHaveLength(1);
    expect(canCleanRegistry(keys, new Set(["x"]))).toBe(false);
    expect(canCleanRegistry(keys, new Set(["HKLM|SOFTWARE\\Foto|64"]))).toBe(true);
  });

  it("rótulos das etapas", () => {
    expect(stageLabel("restore")).toBe("Restauração");
    expect(stageLabel("registry")).toBe("Registro");
    expect(stageLabel("done")).toBe("Concluído");
  });
});

describe("lista de instalados", () => {
  it("mostra programas com tamanho honesto", async () => {
    render(<UninstallHub />);
    expect(await screen.findByText("Foto Studio")).toBeInTheDocument();
    expect(screen.getByText("7-Zip")).toBeInTheDocument();
    const fotoRow = screen.getByText("Foto Studio").closest(".dt-row") as HTMLElement;
    expect(within(fotoRow).getByText("50,0 MB")).toBeInTheDocument();
    expect(within(fotoRow).getByText("estimado")).toBeInTheDocument();
    // Sem estimativa nem pasta medida: "Indisponível", nunca "0 B".
    expect(screen.getByText("Indisponível")).toBeInTheDocument();
    expect(screen.queryByText("0 B")).not.toBeInTheDocument();
  });

  it("declara leitura parcial em vez de fingir lista completa", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_installed_apps"
          ? snapshot({ apps: [], totalCount: 0, partialReason: "HKLM (visão 32) não pôde ser aberta (código 5)" })
          : defaultInvoke(cmd),
      ),
    );
    render(<UninstallHub />);
    expect(await screen.findByText(/Leitura parcial: HKLM/)).toBeInTheDocument();
  });

  it("propaga erro do inventário sem engolir", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      cmd === "list_installed_apps"
        ? Promise.reject("Falha ao abrir o registro")
        : Promise.resolve(defaultInvoke(cmd)),
    );
    render(<UninstallHub />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Falha ao abrir o registro");
  });

  it("mede a pasta de instalação e reordena pelo tamanho real", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "measure_installed_app_sizes") {
        const ids = (args as { appIds: string[] }).appIds;
        return Promise.resolve(
          ids.map(appId =>
            appId === "HKLM|b|64"
              ? { appId, measuredBytes: null, complete: false, fileCount: 0, unavailableReason: "Pasta compartilhada com outro programa" }
              : { appId, measuredBytes: 3 * 1024 ** 3, complete: true, fileCount: 40, installLocation: "C:\\Program Files\\Foto" },
          ),
        );
      }
      return Promise.resolve(defaultInvoke(cmd));
    });
    render(<UninstallHub />);
    expect(await screen.findByText("medido")).toBeInTheDocument();
    const fotoRow = screen.getByText("Foto Studio").closest(".dt-row") as HTMLElement;
    expect(within(fotoRow).getByText("3,00 GB")).toBeInTheDocument();

    // Tamanho decrescente é a ordem padrão: o maior aparece primeiro sem clique.
    const rows = Array.from(document.querySelectorAll(".dt-row")) as HTMLElement[];
    expect(rows[0]).toHaveTextContent("Foto Studio");
    expect(screen.getByRole("button", { name: "Ordenar por tamanho, decrescente" })).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(screen.getByRole("button", { name: "Ordenar por tamanho, decrescente" }));
    expect(screen.getByRole("button", { name: "Ordenar por tamanho, crescente" })).toHaveAttribute("aria-pressed", "true");
  });

  it("mostra dados à parte e mede por completo sob demanda", async () => {
    const quick = { appId: "", measuredBytes: 2 * 1024 ** 3, complete: false, fileCount: 60_000, installLocation: "D:\\Jogos\\Foto", locationSource: "deduced", dataBytes: 512 * 1024 ** 2, dataComplete: true, dataFolders: ["C:\\Users\\u\\AppData\\Roaming\\Foto"] };
    vi.mocked(invoke).mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "measure_installed_app_sizes") {
        const ids = (args as { appIds: string[] }).appIds;
        return Promise.resolve(ids.map(appId => ({ ...quick, appId })));
      }
      if (cmd === "measure_installed_app_size_full") {
        const { appId } = args as { appId: string };
        return Promise.resolve({ ...quick, appId, measuredBytes: 5 * 1024 ** 3, complete: true, fullScan: true });
      }
      return Promise.resolve(defaultInvoke(cmd));
    });
    render(<UninstallHub />);
    const button = await screen.findByRole("button", { name: "Medir Foto Studio por completo" });
    const fotoRow = button.closest(".dt-row") as HTMLElement;
    expect(within(fotoRow).getByText("parcial")).toBeInTheDocument();
    expect(within(fotoRow).getByText(/de dados/)).toHaveTextContent("+ 512,0 MB de dados");

    fireEvent.click(button);
    expect(await within(fotoRow).findByText("deduzido")).toBeInTheDocument();
    expect(within(fotoRow).getByText("5,00 GB")).toBeInTheDocument();
    expect(within(fotoRow).queryByRole("button", { name: /por completo/ })).not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("measure_installed_app_size_full", { appId: app().id });
  });

  it("cada botão Desinstalar pertence à própria linha", async () => {
    render(<UninstallHub />);
    await screen.findByText("Foto Studio");
    const zipRow = screen.getByText("7-Zip").closest(".dt-row") as HTMLElement;
    expect(within(zipRow).getByRole("button", { name: "Desinstalar 7-Zip" })).toBeInTheDocument();
    expect(within(zipRow).queryByRole("button", { name: "Desinstalar Foto Studio" })).not.toBeInTheDocument();
  });

  it("filtra pela busca", async () => {
    render(<UninstallHub />);
    await screen.findByText("Foto Studio");
    fireEvent.change(screen.getByLabelText("Filtrar programas"), { target: { value: "zip" } });
    expect(screen.queryByText("Foto Studio")).not.toBeInTheDocument();
    expect(screen.getByText("7-Zip")).toBeInTheDocument();
  });
});

describe("modal de desinstalação", () => {
  it("abre na etapa de restauração ao clicar no programa", async () => {
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent("Desinstalar Foto Studio");
    expect(screen.getByText("ETAPA 1 DE 4")).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("start_uninstall_session", {
      appId: "HKLM|SOFTWARE\\X\\Uninstall\\Foto|64",
    });
  });

  it("checkbox de restauração desabilitado traz o motivo do Windows", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "get_uninstall_capabilities"
          ? {
              elevated: false,
              restorePointSupported: false,
              restorePointUnavailableReason: "Requer privilégio administrativo",
              verificationAvailable: false,
            }
          : defaultInvoke(cmd),
      ),
    );
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    const checkbox = await screen.findByRole("checkbox", { name: /ponto de restauração/i });
    expect(checkbox).toBeDisabled();
    expect(screen.getAllByText(/Requer privilégio administrativo/).length).toBeGreaterThan(0);
  });

  it("seguir executa o desinstalador padrão com o restore point pedido", async () => {
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("run_app_uninstaller", {
        sessionToken: "sessao-1",
        createRestorePoint: true,
      }),
    );
  });

  it("avança para o registro e mostra o resultado do desinstalador", async () => {
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    expect(await screen.findByText("ETAPA 3 DE 4")).toBeInTheDocument();
    expect(screen.getByText("HKLM\\SOFTWARE\\Foto")).toBeInTheDocument();
    expect(screen.getByText(/ponto de restauração confirmado/i)).toBeInTheDocument();
  });

  it("avisa quando o Windows ainda lista o programa", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "run_app_uninstaller"
          ? outcome({
              entryRemoved: false,
              detectionNote: "A janela do desinstalador fechou, mas o Windows ainda lista este programa.",
            })
          : defaultInvoke(cmd),
      ),
    );
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    expect(await screen.findByText(/ainda lista este programa/)).toBeInTheDocument();
  });

  it("remove chaves marcadas com o id exato vindo do backend", async () => {
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /HKLM.SOFTWARE.Foto/ }));
    fireEvent.click(screen.getByRole("button", { name: /REMOVER 1 CHAVE/ }));
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("cleanup_uninstall_residue", {
        sessionToken: "sessao-1",
        registryIds: ["HKLM|SOFTWARE\\Foto|64"],
        folderPaths: [],
      }),
    );
  });

  it("marcar e desmarcar todas age sobre a lista de chaves", async () => {
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    fireEvent.click(await screen.findByRole("button", { name: "MARCAR TODAS" }));
    expect(await screen.findByRole("button", { name: /REMOVER 1 CHAVE/ })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "DESMARCAR TODAS" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /REMOVER 0 CHAVE/ })).toBeDisabled(),
    );
  });

  it("árvore de pastas seleciona a raiz e mostra o total", async () => {
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    fireEvent.click(await screen.findByRole("button", { name: "IR PARA AS PASTAS" }));
    expect(await screen.findByText("ETAPA 4 DE 4")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: /Foto/ }));
    expect(screen.getByText("1,0 KB")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /REMOVER PASTAS MARCADAS/ }));
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("cleanup_uninstall_residue", {
        sessionToken: "sessao-1",
        registryIds: [],
        folderPaths: ["C:\\Program Files\\Foto"],
      }),
    );
  });

  it("pasta bloqueada não pode ser marcada", async () => {
    const blocked = folder({ children: null, blockedReason: "Link simbólico ou ponto de junção" });
    const blockedTargets = targets({ folders: [blocked] });
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "start_uninstall_session"
          ? session({ targets: blockedTargets })
          : cmd === "run_app_uninstaller"
            ? outcome({ targets: blockedTargets })
            : defaultInvoke(cmd),
      ),
    );
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    fireEvent.click(await screen.findByRole("button", { name: "IR PARA AS PASTAS" }));
    expect(screen.getByText("Link simbólico ou ponto de junção")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /Foto/ })).toBeDisabled();
  });

  it("cancelar devolve a sessão e fecha a modal", async () => {
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: "Cancelar" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("cancel_uninstall_session", {
      sessionToken: "sessao-1",
    });
  });

  it("falha do desinstalador deixa a limpeza de registro disponível", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      cmd === "run_app_uninstaller"
        ? Promise.reject("o Windows exigiu elevação e o Pulse não está como administrador")
        : Promise.resolve(defaultInvoke(cmd)),
    );
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    expect(await screen.findByText(/exigiu elevação/)).toBeInTheDocument();
    expect(screen.getByText("HKLM\\SOFTWARE\\Foto")).toBeInTheDocument();
  });

  it("relata item que o Windows recusou remover", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "cleanup_uninstall_residue"
          ? report({
              registryRemoved: 0,
              items: [
                {
                  id: "HKLM|SOFTWARE\\Foto|64",
                  path: "HKLM\\SOFTWARE\\Foto",
                  removed: false,
                  freedBytes: null,
                  message: "Permissão negada pelo Windows: HKLM exige o Pulse como administrador",
                },
              ],
            })
          : defaultInvoke(cmd),
      ),
    );
    render(<UninstallHub />);
    fireEvent.click(await screen.findByRole("button", { name: "Desinstalar Foto Studio" }));
    fireEvent.click(await screen.findByRole("button", { name: /SEGUIR PARA O DESINSTALADOR/ }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /HKLM.SOFTWARE.Foto/ }));
    fireEvent.click(screen.getByRole("button", { name: /REMOVER 1 CHAVE/ }));
    const summary = await screen.findByRole("status");
    expect(within(summary).getByText(/Permissão negada pelo Windows/)).toBeInTheDocument();
  });
});