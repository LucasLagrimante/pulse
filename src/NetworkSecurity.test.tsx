import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { NetworkSecurity, type NetworkSnapshot, type SecurityPosture } from "./NetworkSecurity";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

function snapshot(overrides: Partial<NetworkSnapshot> = {}): NetworkSnapshot {
  return {
    processes: [
      {
        pid: 4242,
        executablePath: "C:\\Program Files\\Exemplo\\agente.exe",
        processName: "agente.exe",
        establishedCount: 3,
        udpSocketCount: 0,
        listeningCount: 0,
        listensOnAllInterfaces: false,
        destinations: [
          { address: "203.0.113.10", port: 443, isIpv6: false },
          { address: "198.51.100.7", port: 8886, isIpv6: false },
          { address: "2001:db8::1", port: 443, isIpv6: true },
        ],
        externalTotal: 3,
      },
      {
        pid: 5151,
        executablePath: null,
        processName: "Processo protegido (PID 5151)",
        establishedCount: 0,
        udpSocketCount: 2,
        listeningCount: 1,
        listensOnAllInterfaces: true,
        destinations: [],
        externalTotal: 0,
      },
    ],
    establishedTotal: 3,
    establishedExternal: 3,
    establishedLoopback: 0,
    udpTotal: 2,
    listeningTotal: 1,
    listeningPublicTotal: 1,
    unavailableReason: null,
    collectedAtUnixMs: 1_700_000_000_000,
    ...overrides,
  };
}

function posture(overrides: Partial<SecurityPosture> = {}): SecurityPosture {
  return {
    signals: [
      {
        id: "keyboard_filters",
        title: "Filtros de teclado",
        severity: "ok",
        summary: "Somente o driver padrão do Windows (kbdclass)",
        rationale: "Todo driver nessa classe vê cada tecla antes do sistema.",
        evidence: ["UpperFilters: kbdclass"],
      },
      {
        id: "audit_visibility",
        title: "Visibilidade de auditoria",
        severity: "unavailable",
        summary: "Sem privilégio elevado não é possível confirmar a auditoria",
        rationale: "Sem esse log, um processo instalado ontem é invisível hoje.",
        evidence: ["Reinicie o Pulse como administrador para medir este item."],
      },
    ],
    attentionCount: 0,
    unavailableCount: 1,
    collectedAtUnixMs: 1_700_000_000_000,
    elevated: false,
    ...overrides,
  };
}

describe("NetworkSecurity", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation((command: string) => {
      if (command === "get_network_connections") return Promise.resolve(snapshot());
      if (command === "get_security_posture") return Promise.resolve(posture());
      return Promise.reject(new Error(`comando inesperado: ${command}`));
    });
  });

  it("mostra as métricas de conexão e portas expostas", async () => {
    render(<NetworkSecurity />);

    const externas = (await screen.findByText("Conexões externas")).closest("article");
    expect(within(externas as HTMLElement).getByText("3")).toBeInTheDocument();
    expect(within(externas as HTMLElement).getByText("de 3 estabelecidas")).toBeInTheDocument();

    const loopback = screen.getByText("Loopback").closest("article");
    expect(within(loopback as HTMLElement).getByText("0")).toBeInTheDocument();

    const portas = screen.getByText("Portas expostas").closest("article");
    expect(within(portas as HTMLElement).getByText("1")).toBeInTheDocument();
    expect(within(portas as HTMLElement).getByText("de 1 em escuta")).toBeInTheDocument();
  });

  it("renderiza a tabela com os processos retornados pelo backend", async () => {
    render(<NetworkSecurity />);

    expect(await screen.findByText("agente.exe")).toBeInTheDocument();
    expect(screen.getByText("PID 4242")).toBeInTheDocument();
    // Filtro padrão é "Com saída": um processo que só escuta não aparece nele.
    expect(screen.queryByText("Processo protegido (PID 5151)")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Todos" }));
    expect(await screen.findByText("Processo protegido (PID 5151)")).toBeInTheDocument();
  });

  it("expande a linha e lista todos os destinos, inclusive IPv6 entre colchetes", async () => {
    render(<NetworkSecurity />);
    const linha = await screen.findByRole("button", { name: /^agente\.exe, PID 4242/ });

    expect(screen.queryByText("[2001:db8::1]")).not.toBeInTheDocument();

    fireEvent.click(linha);

    expect(await screen.findByText("[2001:db8::1]")).toBeInTheDocument();
    expect(screen.getByText("203.0.113.10")).toBeInTheDocument();
    expect(screen.getByText("C:\\Program Files\\Exemplo\\agente.exe")).toBeInTheDocument();
    expect(linha).toHaveAttribute("aria-expanded", "true");
  });

  it("explica a ausência de caminho em processo protegido em vez de inventar nome", async () => {
    render(<NetworkSecurity />);
    await screen.findByText("agente.exe");
    fireEvent.click(screen.getByRole("button", { name: "Todos" }));
    const linha = await screen.findByRole("button", { name: /^Processo protegido \(PID 5151\)/ });

    fireEvent.click(linha);

    expect(
      await screen.findByText(/Caminho indisponível: processo protegido pelo Windows/),
    ).toBeInTheDocument();
  });

  it("declara a coleta indisponível em vez de exibir zero conexões", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "get_network_connections") {
        return Promise.resolve(
          snapshot({ processes: [], unavailableReason: "GetExtendedTcpTable falhou com 5" }),
        );
      }
      return Promise.resolve(posture());
    });

    render(<NetworkSecurity />);

    expect(await screen.findByText("GetExtendedTcpTable falhou com 5")).toBeInTheDocument();
    expect(screen.getByText("Sem dados de rede para exibir.")).toBeInTheDocument();
  });

  it("marca o sinal de auditoria como não medido quando não há privilégio elevado", async () => {
    render(<NetworkSecurity />);

    expect(
      await screen.findByText("Sem privilégio elevado não é possível confirmar a auditoria"),
    ).toBeInTheDocument();
    expect(screen.getByText("Não medido")).toBeInTheDocument();
    expect(screen.getByText("sem privilégio elevado")).toBeInTheDocument();
  });

  it("destaca os pontos de atenção em faixa própria", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "get_network_connections") return Promise.resolve(snapshot());
      return Promise.resolve(
        posture({
          attentionCount: 1,
          signals: [
            {
              id: "root_certificates",
              title: "Raízes de confiança",
              severity: "attention",
              summary: "69 raízes de terceiros; 3 instaladas desde 2025",
              rationale: "Uma raiz instalada permite assinar certificados aceitos sem aviso.",
              evidence: ["CN=Exemplo (em 2025-01-21)"],
            },
          ],
        }),
      );
    });

    render(<NetworkSecurity />);

    expect(await screen.findByText(/1 ponto exige atenção: Raízes de confiança/)).toBeInTheDocument();
    expect(screen.getByText("Atenção")).toBeInTheDocument();
  });

  it("filtra processos em escuta", async () => {
    render(<NetworkSecurity />);
    await screen.findByText("agente.exe");

    fireEvent.click(screen.getByRole("button", { name: "Em escuta" }));

    await waitFor(() => {
      expect(screen.queryByText("agente.exe")).not.toBeInTheDocument();
    });
    expect(screen.getByText("Processo protegido (PID 5151)")).toBeInTheDocument();
  });
});
