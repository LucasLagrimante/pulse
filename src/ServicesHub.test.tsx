import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ServicesHub,
  categoryLabel,
  categoryTitle,
  countByCategory,
  filterServices,
  formatCount,
  formatElapsed,
  formatPid,
  matchesServiceCategory,
  matchesServiceQuery,
  serviceLabel,
  sortServices,
  sortServicesBy,
  type ServicesSnapshot,
  type UpdatesSnapshot,
  type WindowsService,
} from "./ServicesHub";
import { resetUpgradeJobsForTests } from "./upgradeJobsStore";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

/** Handler registrado do evento de progresso, para simular o backend
 *  durante um teste. `null` significa "nenhum listener registrado". */
let progressHandler: ((event: { payload: { id: string; phase: string; line: string } }) => void) | null =
  null;

function emitProgress(payload: { id: string; phase: string; line: string }): void {
  progressHandler?.({ payload });
}

/** Serviço do Windows em System32 rodando como LocalSystem: o caso comum, e
 *  portanto o que o fixture padrão precisa representar. */
function service(overrides: Partial<WindowsService> = {}): WindowsService {
  return {
    name: "SvcPadrao",
    displayName: "Servico Padrao",
    state: "Em execução",
    stateCode: 4,
    startType: "Automático",
    startTypeCode: 2,
    processId: 1234,
    executablePath: "C:\\Windows\\System32\\svchost.exe",
    isUserService: false,
    icon: null,
    startName: "LocalSystem",
    category: "system",
    publisher: "Microsoft Corporation",
    suspicionReasons: [],
    ...overrides,
  };
}

function servicesSnapshot(overrides: Partial<ServicesSnapshot> = {}): ServicesSnapshot {
  return {
    services: [],
    runningCount: 0,
    stoppedCount: 0,
    pausedCount: 0,
    totalCount: 0,
    unavailableReason: null,
    collectedAtUnixMs: 1,
    ...overrides,
  };
}

function updatesSnapshot(overrides: Partial<UpdatesSnapshot> = {}): UpdatesSnapshot {
  return {
    updates: [],
    totalCount: 0,
    unavailableReason: null,
    collectedAtUnixMs: 1,
    ...overrides,
  };
}

const SAMPLE_SERVICES = [
  service({ name: "AudioSrv", displayName: "Audio do Windows", processId: 900 }),
  service({ name: "Spooler", displayName: "Fila de Impressao", state: "Parado", processId: null }),
  service({ name: "BITS", displayName: "Servico de Transferencia", state: "Pausado", processId: null }),
];

/** Um serviço de cada grupo, para exercitar os chips de categoria. */
const MIXED_SERVICES = [
  service({ name: "AudioSrv", displayName: "Audio do Windows", category: "system" }),
  service({
    name: "MeuAppSvc",
    displayName: "Serviço do MeuApp",
    startName: "DEV-PC\\dev",
    isUserService: true,
    category: "user",
    executablePath: "C:\\Program Files\\MeuApp\\servico.exe",
    publisher: "MeuApp Ltda",
  }),
  service({
    name: "ServicoSemDados",
    category: "unknown",
    startName: null,
    publisher: null,
  }),
  service({
    name: "AchadoNaReview",
    displayName: "Serviço em diretório gravável",
    startName: "DEV-PC\\dev",
    isUserService: true,
    category: "review",
    executablePath: "C:\\Users\\dev\\AppData\\Roaming\\algo\\svc.exe",
    publisher: null,
    suspicionReasons: ["Executável em diretório gravável pelo usuário (c:\\users\\dev\\appdata\\roaming)"],
  }),
];

const SAMPLE_SERVICES_SNAPSHOT = servicesSnapshot({
  services: SAMPLE_SERVICES,
  runningCount: 1,
  stoppedCount: 1,
  pausedCount: 1,
  totalCount: 3,
});

/** Mock padrão: qualquer comando desconhecido devolve um objeto válido, para
 *  não quebrar outros testes que compartilham este módulo. */
function defaultInvoke(cmd: string): unknown {
  if (cmd === "list_windows_services") return servicesSnapshot({ services: SAMPLE_SERVICES, runningCount: 1, stoppedCount: 1, pausedCount: 1, totalCount: 3 });
  if (cmd === "list_pending_updates") return updatesSnapshot();
  if (cmd === "set_windows_service_state") return SAMPLE_SERVICES_SNAPSHOT;
  if (cmd === "upgrade_winget_package") return "atualizado";
  if (cmd === "upgrade_all_winget_packages") return "atualizado";
  return {};
}

beforeEach(() => {
  // O progresso das instalações vive em escopo de módulo; zera entre casos.
  resetUpgradeJobsForTests();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string) => Promise.resolve(defaultInvoke(cmd)));
  progressHandler = null;
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(((_event: unknown, handler: unknown) => {
    progressHandler = handler as typeof progressHandler;
    return Promise.resolve(() => {});
  }) as unknown as typeof listen);
});

// Sem isto, o DOM do teste anterior permanece e as buscas por role/text
// encontram nós de outra renderização — falha que parece intermitente.
afterEach(() => cleanup());

describe("helpers puros", () => {
  it("formatCount nunca transforma ausência em zero", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(1234)).toBe("1.234");
    expect(formatCount(null)).toBe("Indisponível");
    expect(formatCount(Number.NaN)).toBe("Indisponível");
  });

  it("formatPid só mostra PID para serviço em execução", () => {
    expect(formatPid(service({ processId: 77 }))).toBe("PID 77");
    expect(formatPid(service({ state: "Parado", processId: null }))).toBe("Processo inativo");
    expect(formatPid(service({ processId: null }))).toBe("PID indisponível");
  });

  it("filterServices combina busca e filtro de estado", () => {
    expect(filterServices(SAMPLE_SERVICES, "", "all")).toHaveLength(3);
    expect(filterServices(SAMPLE_SERVICES, "", "running").map(s => s.name)).toEqual(["AudioSrv"]);
    expect(filterServices(SAMPLE_SERVICES, "", "stopped").map(s => s.name)).toEqual(["Spooler"]);
    expect(filterServices(SAMPLE_SERVICES, "", "paused").map(s => s.name)).toEqual(["BITS"]);
    expect(filterServices(SAMPLE_SERVICES, "audio", "all").map(s => s.name)).toEqual(["AudioSrv"]);
    // Busca no caminho do executável, sem acento no displayName.
    expect(matchesServiceQuery(service(), "system32")).toBe(true);
    expect(filterServices(SAMPLE_SERVICES, "fila", "all").map(s => s.name)).toEqual(["Spooler"]);
    // Busca que não casa com nada devolve lista vazia, não a lista inteira.
    expect(filterServices(SAMPLE_SERVICES, "inexistente", "all")).toHaveLength(0);
  });

  it("sortServices põe Em execução primeiro e não muta a entrada", () => {
    const input = [service({ name: "B", state: "Parado" }), service({ name: "A", state: "Em execução" })];
    const sorted = sortServices(input);
    expect(sorted[0].name).toBe("A");
    expect(input[0].name).toBe("B");
    expect(serviceLabel(service({ displayName: "", name: "Fallback" }))).toBe("Fallback");
  });

  it("categoryLabel nunca usa palavra de acusação", () => {
    expect(categoryLabel("system")).toBe("Sistema");
    expect(categoryLabel("user")).toBe("Usuário");
    expect(categoryLabel("review")).toBe("Revisão");
    expect(categoryLabel("unknown")).toBe("Sem dados");
    const rotulos = (["system", "user", "review", "unknown"] as const)
      .map(category => categoryLabel(category))
      .join(" ")
      .toLowerCase();
    for (const proibida of ["malware", "vírus", "suspeito", "infectado", "perigoso"]) {
      expect(rotulos).not.toContain(proibida);
    }
  });

  it("filtro de categoria separa sistema, usuário e revisão", () => {
    expect(filterServices(MIXED_SERVICES, "", "all").length).toBe(4);
    expect(filterServices(MIXED_SERVICES, "", "all", "system").map(s => s.name)).toEqual(["AudioSrv"]);
    expect(filterServices(MIXED_SERVICES, "", "all", "review").map(s => s.name)).toEqual(["AchadoNaReview"]);
    expect(filterServices(MIXED_SERVICES, "", "all", "unknown").map(s => s.name)).toEqual(["ServicoSemDados"]);
    // "user" usa isUserService: o item em revisão continua sendo do usuário.
    expect(filterServices(MIXED_SERVICES, "", "all", "user").map(s => s.name).sort()).toEqual([
      "AchadoNaReview",
      "MeuAppSvc",
    ]);
    expect(matchesServiceCategory(service(), "all")).toBe(true);
  });

  it("categoria combina com o filtro de estado, não o substitui", () => {
    const revisaoParada = service({
      name: "ParadaEmRevisao",
      category: "review",
      state: "Parado",
      stateCode: 1,
      processId: null,
      isUserService: true,
      suspicionReasons: ["Executável em diretório gravável pelo usuário (c:\\temp)"],
    });
    const lista = [revisaoParada, ...MIXED_SERVICES];
    expect(filterServices(lista, "", "stopped", "review").map(s => s.name)).toEqual(["ParadaEmRevisao"]);
    expect(filterServices(lista, "", "running", "review").map(s => s.name)).toEqual(["AchadoNaReview"]);
  });

  it("countByCategory soma a lista inteira sem perder serviço", () => {
    const counts = countByCategory(MIXED_SERVICES);
    expect(counts).toEqual({ system: 1, user: 1, review: 1, unknown: 1 });
    expect(Object.values(counts).reduce((a, b) => a + b, 0)).toBe(MIXED_SERVICES.length);
    // Categoria vinda do futuro não pode quebrar a contagem.
    const futuro = service({ category: "inventada" as WindowsService["category"] });
    expect(Object.values(countByCategory([futuro])).reduce((a, b) => a + b, 0)).toBe(1);
  });

  it("a busca acha por conta de execução e por motivo de revisão", () => {
    expect(matchesServiceQuery(service({ startName: "NT AUTHORITY\\SYSTEM" }), "authority")).toBe(true);
    expect(matchesServiceQuery(MIXED_SERVICES[3], "gravável")).toBe(true);
    expect(filterServices(MIXED_SERVICES, "meuapp", "all").map(s => s.name)).toEqual(["MeuAppSvc"]);
  });

  it("categoryTitle declara a evidência e admite a ausência de dado", () => {
    expect(categoryTitle(service())).toContain("conta: LocalSystem");
    expect(categoryTitle(MIXED_SERVICES[3])).toContain("sinais:");
    const semDados = categoryTitle(MIXED_SERVICES[2]);
    expect(semDados).toContain("Sem dados");
    expect(semDados).toContain("não informou");
  });
});

/** Abre a aba de serviços. A aba inicial é Atualizações, então todo teste da
 *  aba de serviços precisa trocar explicitamente — e o badge de contagem na aba
 *  garante que a troca é visível, não implícita. */
async function abrirAbaServicos(): Promise<void> {
  fireEvent.click(screen.getByRole("tab", { name: /SERVIÇOS/i }));
  await waitFor(() => expect(screen.getByRole("tab", { name: /SERVIÇOS/i }).getAttribute("aria-selected")).toBe("true"));
}

describe("Aba de serviços", () => {
  it("abre em Atualizações e deixa Serviços como segunda aba", async () => {
    render(<ServicesHub />);
    // Ordem exigida: Atualizações primeiro.
    const tabs = screen.getAllByRole("tab");
    expect(tabs[0].textContent).toContain("ATUALIZAÇÕES");
    expect(tabs[1].textContent).toContain("SERVIÇOS");
    await waitFor(() => expect(tabs[0].getAttribute("aria-selected")).toBe("true"));
    // Badge de contagem nas duas abas, no mesmo padrão do Armazenamento.
    // O mock padrão de atualizações é `totalCount: 0`, então o badge mostra 0
    // — ainda assim prova que a contagem está sendo lida do snapshot, e não
    // hard-coded.
    await waitFor(() => expect(within(tabs[1]).getByText("3")).toBeTruthy());
    expect(within(tabs[0]).getByText("0")).toBeTruthy();
  });

  it("renderiza os dados e os totais nos cards", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    expect(screen.getByText("Fila de Impressao")).toBeTruthy();
    const resumo = within(screen.getByLabelText("Resumo dos serviços"));
    expect(within(resumo.getByText("Total").parentElement as HTMLElement).getByText("3")).toBeTruthy();
    expect(within(resumo.getByText("Em execução").parentElement as HTMLElement).getByText("1")).toBeTruthy();
    expect(resumo.getByText("Parados")).toBeTruthy();
    expect(resumo.getByText("Pausados")).toBeTruthy();
  });

  it("mostra PID apenas do serviço em execução", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    expect(screen.getByText("PID 900")).toBeTruthy();
    expect(screen.getAllByText("Processo inativo")).toHaveLength(2);
  });

  it("filtra por estado através do select", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    fireEvent.change(screen.getByLabelText("Filtrar serviços por estado"), { target: { value: "stopped" } });
    expect(screen.queryByText("Audio do Windows")).toBeNull();
    expect(screen.getByText("Fila de Impressao")).toBeTruthy();
  });

  it("busca por nome", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    fireEvent.change(screen.getByLabelText("Buscar serviços por nome ou executável"), { target: { value: "bits" } });
    expect(screen.queryByText("Audio do Windows")).toBeNull();
    expect(screen.getByText("BITS")).toBeTruthy();
  });

  it("separa os serviços em Sistema, Usuário e Revisão pelos chips", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_windows_services"
          ? servicesSnapshot({ services: MIXED_SERVICES, totalCount: 4 })
          : defaultInvoke(cmd),
      ),
    );
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());

    const grupo = screen.getByRole("group", { name: "Filtrar serviços por origem" });
    // Todo grupo aparece com contagem, inclusive os zerados — esconder "Revisão:
    // 0" seria esconder que a máquina não tem sinal nenhum.
    for (const rotulo of ["Sistema", "Usuário", "Revisão", "Sem dados"]) {
      expect(within(grupo).getByRole("button", { name: new RegExp(rotulo) })).toBeTruthy();
    }
    const revisao = within(grupo).getByRole("button", { name: /Revisão/ });
    expect(revisao.getAttribute("aria-pressed")).toBe("false");
    expect(revisao.textContent).toContain("1");

    fireEvent.click(revisao);
    await waitFor(() => expect(revisao.getAttribute("aria-pressed")).toBe("true"));
    expect(screen.getByText("Serviço em diretório gravável")).toBeTruthy();
    expect(screen.queryByText("Audio do Windows")).toBeNull();
    expect(screen.queryByText("Serviço do MeuApp")).toBeNull();

    fireEvent.click(within(grupo).getByRole("button", { name: /Usuário/ }));
    await waitFor(() => expect(screen.getByText("Serviço do MeuApp")).toBeTruthy());
    // Revisão vence como categoria, mas o serviço continua sendo do usuário.
    expect(screen.getByText("Serviço em diretório gravável")).toBeTruthy();
  });

  it("mostra o motivo verificável de cada serviço em revisão", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_windows_services"
          ? servicesSnapshot({ services: MIXED_SERVICES, totalCount: 4 })
          : defaultInvoke(cmd),
      ),
    );
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Serviço em diretório gravável")).toBeTruthy());

const motivos = screen.getByLabelText("Sinais que exigem revisão");
    expect(motivos.textContent).toContain("gravável pelo usuário");
    // Só o serviço com sinal mostra motivos — os outros três não ganham lista.
    expect(screen.getAllByLabelText("Sinais que exigem revisão")).toHaveLength(1);

    // Badge por item: um para cada serviço visível, e o texto do badge é o
    // rótulo do grupo (nunca "suspeito"). A lista é localizada a partir dos
    // próprios motivos porque as duas abas têm uma `role="list"`.
    const lista = motivos.closest('[role="list"]') as HTMLElement;
    const itens = within(lista);
    expect(itens.getAllByText("Revisão")).toHaveLength(1);
    expect(itens.getAllByText("Sistema")).toHaveLength(1);
    expect(itens.getAllByText("Sem dados")).toHaveLength(1);
    expect(itens.queryAllByText(/suspeito|malware|vírus/i)).toHaveLength(0);
  });

  it("exibe unavailableReason em vez de fingir zero serviços", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_windows_services"
          ? servicesSnapshot({ unavailableReason: "Acesso ao Gerenciador de Servicos negado pelo sistema" })
          : defaultInvoke(cmd),
      ),
    );
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Acesso ao Gerenciador de Servicos negado"),
    );
    expect(screen.queryByText("Nenhum serviço corresponde aos filtros aplicados.")).toBeNull();
  });

  it("mostra estado vazio quando nada corresponde aos filtros", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    fireEvent.change(screen.getByLabelText("Buscar serviços por nome ou executável"), { target: { value: "zzzz" } });
    expect(screen.getByText("Nenhum serviço corresponde aos filtros aplicados.")).toBeTruthy();
  });

  it("acao PARAR chama set_windows_service_state com stop", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    const row = screen.getByText("Audio do Windows").closest("li") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: /^Parar / }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_windows_service_state", { name: "AudioSrv", action: "stop" }),
    );
  });

  it("acao INICIAR chama set_windows_service_state com start", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Fila de Impressao")).toBeTruthy());
    const row = screen.getByText("Fila de Impressao").closest("li") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: /^Iniciar / }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_windows_service_state", { name: "Spooler", action: "start" }),
    );
  });

  it("exibe o erro de acesso negado devolvido pelo Windows", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      cmd === "set_windows_service_state"
        ? Promise.reject("Acesso negado. Execute o Pulse como administrador.")
        : Promise.resolve(defaultInvoke(cmd)),
    );
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    const row = screen.getByText("Audio do Windows").closest("li") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: /^Parar / }));
    await waitFor(() =>
      expect(screen.getAllByRole("alert").some(node => node.textContent?.includes("Acesso negado"))).toBe(true),
    );
  });
});

describe("Aba de atualizações", () => {
  /** Atualizações é a aba inicial: não há mais troca de aba para fazer.
   *  Mantido como função (em vez de `render` direto) porque os testes
   *  chamavam `openUpdatesTab()` e ler o retorno. */
  async function openUpdatesTab() {
    const view = render(<ServicesHub />);
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /ATUALIZAÇÕES/i }).getAttribute("aria-selected")).toBe(
        "true",
      ),
    );
    return view;
  }

  it("lista atualizações com versões e origem", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_pending_updates"
          ? updatesSnapshot({
              updates: [
                { id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" },
                { id: "Loja.App", name: "App da Loja", currentVersion: "", availableVersion: "2.0", source: "msstore" },
              ],
              totalCount: 2,
            })
          : defaultInvoke(cmd),
      ),
    );
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    expect(screen.getByText("4.35.1")).toBeTruthy();
    expect(screen.getByText("4.36.0")).toBeTruthy();
    expect(screen.getByText("Docker.DockerDesktop")).toBeTruthy();
    expect(screen.getByText("Origem: winget")).toBeTruthy();
    // Versão ausente nunca aparece como string vazia nem como 0.
    expect(screen.getByText("Indisponível")).toBeTruthy();
  });

  it("exibe unavailableReason das atualizações", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_pending_updates"
          ? updatesSnapshot({ unavailableReason: "Gerenciador de Pacotes do Windows (winget) não encontrado neste sistema" })
          : defaultInvoke(cmd),
      ),
    );
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("winget) não encontrado"));
  });

  it("botão ATUALIZAR invoca upgrade_winget_package com o id", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_pending_updates"
          ? updatesSnapshot({
              updates: [{ id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" }],
              totalCount: 1,
            })
          : defaultInvoke(cmd),
      ),
    );
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: /^Atualizar / }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("upgrade_winget_package", { id: "Docker.DockerDesktop" }),
    );
    // O sucesso é por item, não um aviso global: o clique em outro pacote não
    // pode apagar o estado do que já terminou.
    await waitFor(() => expect(screen.getByText("ATUALIZADO")).toBeTruthy());
  });

  it("exibe o erro do upgrade sem engolir a mensagem", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_pending_updates") {
        return Promise.resolve(
          updatesSnapshot({
            updates: [{ id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" }],
            totalCount: 1,
          }),
        );
      }
      if (cmd === "upgrade_winget_package") return Promise.reject("Acesso negado ao instalar o pacote");
      return Promise.resolve(defaultInvoke(cmd));
    });
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: /^Atualizar / }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Acesso negado ao instalar o pacote"),
    );
  });

  it("mostra estado vazio quando não há atualizações", async () => {
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Nenhuma atualização pendente no momento.")).toBeTruthy());
  });

  it("ATUALIZAR TUDO fica desabilitado quando não há pacotes pendentes", async () => {
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Nenhuma atualização pendente no momento.")).toBeTruthy());
    expect(screen.getByRole("button", { name: "ATUALIZAR TUDO" })).toHaveProperty("disabled", true);
  });

  it("ATUALIZAR TUDO invoca upgrade_all_winget_packages e reconsulta a lista", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "upgrade_all_winget_packages"
          ? { total: 2, succeeded: ["Docker.DockerDesktop", "Anki.Anki"], failed: [] }
          : cmd === "list_pending_updates"
          ? updatesSnapshot({
              updates: [
                { id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" },
                { id: "Anki.Anki", name: "Anki Launcher", currentVersion: "25.09", availableVersion: "26.09.3", source: "winget" },
              ],
              totalCount: 2,
            })
          : defaultInvoke(cmd),
      ),
    );
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    const before = vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length;
    fireEvent.click(screen.getByRole("button", { name: "ATUALIZAR TUDO" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("upgrade_all_winget_packages"));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("Todos os 2 aplicativos pendentes foram atualizados"),
    );
    // A lista é reconsultada após a execução em lote.
    const after = vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length;
    expect(after).toBeGreaterThan(before);
  });

  it("mantém o progresso do lote ao sair e voltar para a área", async () => {
    let finishBatch: (value: string) => void = () => {};
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_pending_updates") {
        return Promise.resolve(
          updatesSnapshot({
            updates: [{ id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" }],
            totalCount: 1,
          }),
        );
      }
      if (cmd === "upgrade_all_winget_packages") {
        return new Promise<string>(resolve => {
          finishBatch = resolve;
        });
      }
      return Promise.resolve(defaultInvoke(cmd));
    });
    const first = await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "ATUALIZAR TUDO" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("upgrade_all_winget_packages"));

    // Troca de área: o hub é desmontado, mas o winget segue emitindo.
    first.unmount();
    act(() => progressHandler?.({ payload: { id: "*", phase: "log", line: "Instalando Docker Desktop" } }));

    await openUpdatesTab();
    expect(screen.getByRole("button", { name: "ATUALIZANDO TUDO…" })).toHaveProperty("disabled", true);
    expect(screen.getByText("Instalando Docker Desktop")).toBeTruthy();

    const before = vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length;
    await act(async () => finishBatch("ok"));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("Todos os aplicativos desatualizados"),
    );
    expect(vi.mocked(invoke).mock.calls.filter(call => call[0] === "upgrade_all_winget_packages")).toHaveLength(1);
    expect(
      vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length,
    ).toBeGreaterThan(before);
  });

  it("propaga o erro da atualização em lote", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "list_pending_updates") {
        return Promise.resolve(
          updatesSnapshot({
            updates: [{ id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" }],
            totalCount: 1,
          }),
        );
      }
      if (cmd === "upgrade_all_winget_packages") return Promise.reject("Acesso negado ao instalar o pacote");
      return Promise.resolve(defaultInvoke(cmd));
    });
    await openUpdatesTab();
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "ATUALIZAR TUDO" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Acesso negado ao instalar o pacote"),
    );
  });
  describe("falhas estruturadas do winget", () => {
    const TWO = () =>
      updatesSnapshot({
        updates: [
          { id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" },
          { id: "Anki.Anki", name: "Anki Launcher", currentVersion: "25.09", availableVersion: "26.09.3", source: "winget" },
        ],
        totalCount: 2,
      });
    const ADMIN_FAILURE = {
      message: "O pacote registra um serviço do Windows e exige privilégios de administrador",
      code: "0x80073D28",
      needsAdmin: true,
      detail: null,
    };

    it("lote com falhas lista a causa por pacote em vez da saída crua do winget", async () => {
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === "list_pending_updates") return Promise.resolve(TWO());
        if (cmd === "upgrade_all_winget_packages") {
          return Promise.resolve({
            total: 2,
            succeeded: ["Docker.DockerDesktop"],
            failed: [{ id: "Anki.Anki", name: "Anki Launcher", ...ADMIN_FAILURE }],
          });
        }
        return Promise.resolve(defaultInvoke(cmd));
      });
      await openUpdatesTab();
      await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
      fireEvent.click(screen.getByRole("button", { name: "ATUALIZAR TUDO" }));

      await waitFor(() => expect(screen.getByText("1 de 2 aplicativos não foi atualizado")).toBeTruthy());
      const report = screen.getByText("1 de 2 aplicativos não foi atualizado").closest("[role=alert]");
      expect(report?.textContent).toContain("Anki Launcher");
      expect(report?.textContent).toContain("exige privilégios de administrador");
      expect(report?.textContent).toContain("0x80073D28");
      expect(screen.getByText("1 de 2 aplicativos atualizados.")).toBeTruthy();

      fireEvent.click(screen.getByRole("button", { name: /TENTAR COMO ADMINISTRADOR/ }));
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith("upgrade_winget_package_elevated", { id: "Anki.Anki" }),
      );
      // Sucesso como administrador tira o pacote do relatório.
      await waitFor(() => expect(screen.queryByText("1 de 2 aplicativos não foi atualizado")).toBeNull());
    });

    it("falha que exige administrador oferece repetir elevado na própria linha", async () => {
      vi.mocked(invoke).mockImplementation((cmd: string) => {
        if (cmd === "list_pending_updates") return Promise.resolve(TWO());
        if (cmd === "upgrade_winget_package") return Promise.reject(ADMIN_FAILURE);
        return Promise.resolve(defaultInvoke(cmd));
      });
      await openUpdatesTab();
      await waitFor(() => expect(screen.getByText("Anki Launcher")).toBeTruthy());
      fireEvent.click(screen.getByRole("button", { name: "Atualizar Anki Launcher" }));

      await waitFor(() =>
        expect(screen.getByRole("alert").textContent).toBe(
          "O pacote registra um serviço do Windows e exige privilégios de administrador (0x80073D28)",
        ),
      );
      fireEvent.click(screen.getByRole("button", { name: "Atualizar Anki Launcher como administrador" }));
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith("upgrade_winget_package_elevated", { id: "Anki.Anki" }),
      );
    });
});
});

describe("Atualizar tudo na aba de serviços", () => {
  it("reconsulta serviços e atualizações em paralelo", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    vi.mocked(invoke).mockClear();
    fireEvent.click(screen.getByRole("button", { name: "ATUALIZAR TUDO" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_windows_services", { forceRefresh: true }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_pending_updates", { forceRefresh: true }));
  });
});

describe("atualizações em paralelo", () => {
  const TWO_UPDATES = updatesSnapshot({
    updates: [
      { id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" },
      { id: "Anki.Anki", name: "Anki Launcher", currentVersion: "25.09", availableVersion: "26.09.3", source: "winget" },
    ],
    totalCount: 2,
  });

  /** Promise controlável por pacote: o teste decide quando cada instalação
   *  termina, que é o que permite observar dois loadings simultâneos. */
  function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: string) => void;
    const promise = new Promise<T>((res, rej) => {
      resolve = res;
      reject = rej;
    });
    return { promise, resolve, reject };
  }

  async function openTwoUpdates(upgrade: (id: string) => Promise<string>) {
    vi.mocked(invoke).mockImplementation(((cmd: string, args?: { id?: string }) =>
      cmd === "upgrade_winget_package"
        ? upgrade(args?.id ?? "")
        : Promise.resolve(cmd === "list_pending_updates" ? TWO_UPDATES : defaultInvoke(cmd))) as typeof invoke);
    const view = render(<ServicesHub />);
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    await waitFor(() => expect(screen.getByText("Anki Launcher")).toBeTruthy());
    return view;
  }

  /** Cria uma promessa controlável por pacote e devolve o registro. */
  function jobTracker() {
    const jobs: Record<string, ReturnType<typeof deferred<string>>> = {};
    const upgrade = (id: string) => {
      const job = deferred<string>();
      jobs[id] = job;
      return job.promise;
    };
    return { jobs, upgrade };
  }

  it("mantém o loading do primeiro pacote quando o segundo é clicado", async () => {
    const { jobs, upgrade } = jobTracker();
    await openTwoUpdates(upgrade);

    const botoes = screen.getAllByRole("button", { name: /^Atualizar / }) as HTMLButtonElement[];
    fireEvent.click(botoes[0]);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("upgrade_winget_package", { id: "Docker.DockerDesktop" }),
    );
    fireEvent.click(botoes[1]);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("upgrade_winget_package", { id: "Anki.Anki" }),
    );

    // Ambos em execução ao mesmo tempo, cada um com a sua barra.
    expect(screen.getByRole("progressbar", { name: /Docker Desktop em andamento/ })).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: /Anki Launcher em andamento/ })).toBeTruthy();
    expect(screen.getByText("2 atualizações em segundo plano")).toBeTruthy();

    // Concluir o primeiro NÃO apaga o loading do segundo.
    await act(async () => {
      jobs["Docker.DockerDesktop"].resolve("ok");
    });
    await waitFor(() => expect(screen.getByText("ATUALIZADO")).toBeTruthy());
    expect(screen.getByRole("progressbar", { name: /Anki Launcher em andamento/ })).toBeTruthy();
    expect(screen.queryByText("2 atualizações em segundo plano")).toBeNull();
    expect(screen.getByText("1 atualização em segundo plano")).toBeTruthy();
  });

  it("mostra a linha de log do winget no item que está instalando", async () => {
    const job = deferred<string>();
    await openTwoUpdates(() => job.promise);
    fireEvent.click(screen.getAllByRole("button", { name: /^Atualizar / })[0]);
    await waitFor(() =>
      expect(screen.getByRole("progressbar", { name: /Docker Desktop em andamento/ })).toBeTruthy(),
    );
    act(() => {
      emitProgress({ id: "Docker.DockerDesktop", phase: "log", line: "Downloading Docker Desktop 4.36.0" });
    });
    await waitFor(() => expect(screen.getByText("Downloading Docker Desktop 4.36.0")).toBeTruthy());
    await act(async () => {
      job.resolve("ok");
    });
    await waitFor(() => expect(screen.getByText("ATUALIZADO")).toBeTruthy());
  });

  it("mantém o erro do primeiro pacote quando o segundo termina", async () => {
    const { jobs, upgrade } = jobTracker();
    await openTwoUpdates(upgrade);
    const botoes = screen.getAllByRole("button", { name: /^Atualizar / }) as HTMLButtonElement[];
    fireEvent.click(botoes[0]);
    fireEvent.click(botoes[1]);
    await waitFor(() => expect(jobs["Anki.Anki"]).toBeTruthy());
    await act(async () => {
      jobs["Docker.DockerDesktop"].reject("Acesso negado ao instalar o pacote");
      jobs["Anki.Anki"].resolve("ok");
    });
    await waitFor(() => expect(screen.getByText("Acesso negado ao instalar o pacote")).toBeTruthy());
    expect(screen.getByText("ATUALIZADO")).toBeTruthy();
  });

  it("ignora clique repetido no mesmo pacote enquanto ele instala", async () => {
    const job = deferred<string>();
    await openTwoUpdates(() => job.promise);
    const botao = screen.getAllByRole("button", { name: /^Atualizar / })[0];
    fireEvent.click(botao);
    fireEvent.click(botao);
    fireEvent.click(botao);
    await waitFor(() => expect(botao).toHaveProperty("disabled", true));
    const calls = vi.mocked(invoke).mock.calls.filter(call => call[0] === "upgrade_winget_package");
    expect(calls).toHaveLength(1);
    await act(async () => {
      job.resolve("ok");
    });
    await waitFor(() => expect(screen.getByText("ATUALIZADO")).toBeTruthy());
  });

  it("só reconsulta a lista depois que a última instalação termina", async () => {
    const { jobs, upgrade } = jobTracker();
    await openTwoUpdates(upgrade);
    const botoes = screen.getAllByRole("button", { name: /^Atualizar / }) as HTMLButtonElement[];
    fireEvent.click(botoes[0]);
    fireEvent.click(botoes[1]);
    await waitFor(() => expect(jobs["Anki.Anki"]).toBeTruthy());

    const before = vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length;
    await act(async () => {
      jobs["Docker.DockerDesktop"].resolve("ok");
    });
    await waitFor(() => expect(screen.getByText("ATUALIZADO")).toBeTruthy());
    expect(
      vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length,
    ).toBe(before);

    await act(async () => {
      jobs["Anki.Anki"].resolve("ok");
    });
    await waitFor(() =>
      expect(
        vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length,
      ).toBeGreaterThan(before),
    );
  });

  it("formatElapsed conta minutos e segundos", () => {
    expect(formatElapsed(0)).toBe("00:00");
    expect(formatElapsed(65_000)).toBe("01:05");
    expect(formatElapsed(600_000)).toBe("10:00");
  });

  it("exibe skeletons e banner de loading enquanto consulta atualizações", async () => {
    const pendente = new Promise(() => {});
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      cmd === "list_pending_updates" ? pendente : Promise.resolve(defaultInvoke(cmd)),
    );
    render(<ServicesHub />);
    await waitFor(() => {
      expect(screen.getByLabelText("Carregando lista de atualizações")).toBeTruthy();
    });
    expect(screen.getByText(/Consultando repositório oficial/i)).toBeTruthy();
  });

  it("campo de busca na aba de atualizações filtra itens por nome ou ID", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_pending_updates"
          ? updatesSnapshot({
              updates: [
                { id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" },
                { id: "Anki.Anki", name: "Anki Launcher", currentVersion: "25.09", availableVersion: "26.09.3", source: "winget" },
              ],
              totalCount: 2,
            })
          : defaultInvoke(cmd),
      ),
    );
    render(<ServicesHub />);
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());
    expect(screen.getByText("Anki Launcher")).toBeTruthy();

    const searchInput = screen.getByLabelText("Buscar aplicativo por nome, ID ou versão");
    fireEvent.change(searchInput, { target: { value: "anki" } });

    expect(screen.queryByText("Docker Desktop")).toBeNull();
    expect(screen.getByText("Anki Launcher")).toBeTruthy();

    fireEvent.change(searchInput, { target: { value: "inexistente" } });
    expect(screen.getByText("Nenhum aplicativo corresponde aos filtros aplicados.")).toBeTruthy();
  });

  it("botão VERIFICAR reconsulta a lista de atualizações pendentes", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "list_pending_updates"
          ? updatesSnapshot({
              updates: [
                { id: "Docker.DockerDesktop", name: "Docker Desktop", currentVersion: "4.35.1", availableVersion: "4.36.0", source: "winget" },
              ],
              totalCount: 1,
            })
          : defaultInvoke(cmd),
      ),
    );
    render(<ServicesHub />);
    await waitFor(() => expect(screen.getByText("Docker Desktop")).toBeTruthy());

    const before = vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length;
    const btnVerificar = screen.getByRole("button", { name: "VERIFICAR" });
    fireEvent.click(btnVerificar);

    await waitFor(() => {
      const after = vi.mocked(invoke).mock.calls.filter(call => call[0] === "list_pending_updates").length;
      expect(after).toBeGreaterThan(before);
    });
  });
});
describe("ordenação por coluna", () => {
  it("sortServicesBy ordena por nome, origem e estado com desempate alfabético", () => {
    const byName = sortServicesBy(SAMPLE_SERVICES, "name", "asc").map(s => serviceLabel(s));
    expect(byName).toEqual(["Audio do Windows", "Fila de Impressao", "Servico de Transferencia"]);
    expect(sortServicesBy(SAMPLE_SERVICES, "name", "desc")[0].name).toBe("BITS");
    // Revisão primeiro na origem crescente: o sinal aparece no topo.
    expect(sortServicesBy(MIXED_SERVICES, "category", "asc")[0].name).toBe("AchadoNaReview");
    expect(sortServicesBy(SAMPLE_SERVICES, "state", "asc").map(s => s.name)).toEqual(sortServices(SAMPLE_SERVICES).map(s => s.name));
  });

  it("clicar no cabeçalho Serviço reordena a lista e alterna a direção", async () => {
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Ordenar por serviço" }));
    fireEvent.click(screen.getByRole("button", { name: "Ordenar por serviço, crescente" }));
    const first = document.querySelector(".dt--services .dt-row strong");
    expect(first?.textContent).toBe("Servico de Transferencia");
  });
});

describe("revisão de segurança com IA", () => {
  it("o botão da aba Serviços abre o modal e gera o prompt sem alterar nada", async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "detect_ai_cli_providers") return Promise.resolve(["claude"]);
      if (cmd === "get_ai_services_review_status") {
        return Promise.resolve({ path: "C:\\Temp\\pulse-services-security-review.md", exists: false, modifiedAtUnixMs: null, sizeBytes: 0 });
      }
      if (cmd === "get_ai_services_analysis_preview") {
        return Promise.resolve({
          provider: "claude",
          providerName: "Claude Code",
          command: "claude 'Faça uma revisão'",
          prompt: "Faça uma revisão de segurança dos serviços",
          contextFilePath: "C:\\Temp\\pulse-services-security-context.md",
          runningCount: 120,
          analyzedCount: 14,
          reviewCount: 0,
          withoutPublisherCount: 2,
          highlighted: [],
          collectedAtUnixMs: 1,
          isAvailable: true,
          environment: "Windows (nativo)",
        });
      }
      return Promise.resolve(defaultInvoke(cmd));
    });
    render(<ServicesHub />);
    await abrirAbaServicos();
    await waitFor(() => expect(screen.getByText("Audio do Windows")).toBeTruthy());

    fireEvent.click(await screen.findByRole("button", { name: /REVISAR SEGURANÇA COM/ }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_ai_services_analysis_preview", { provider: "claude", customPrompt: null }),
    );
    expect(await screen.findByText("Faça uma revisão de segurança dos serviços")).toBeTruthy();
    expect(screen.getByText("14")).toBeTruthy();
    // Gerar o prompt nunca dispara ação de serviço.
    expect(invoke).not.toHaveBeenCalledWith("set_windows_service_state", expect.anything());
  });
});
