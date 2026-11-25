import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SecurityHub, type RemoteAccessAudit } from "./SecurityHub";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockedInvoke = vi.mocked(invoke);

function makeWslgOnlyAudit(): RemoteAccessAudit {
  return {
    verdict: "wslg_only",
    verdictTitle: "Sem indício de acesso remoto — apenas integração interna do WSLg",
    verdictSummary:
      "RDP está desabilitado, não há sessões remotas nem agentes de terceiros. Os componentes Remote Desktop encontrados pertencem ao WSLg local.",
    rdpConfig: {
      enabled: false,
      configuredPort: 3389,
      isCustomPort: false,
      nlaRequired: true,
      serviceStatus: "Parado",
      servicePid: null,
      unavailableReason: null,
    },
    sessions: [
      {
        sessionName: "console",
        username: "lucas",
        sessionId: 1,
        state: "Active",
        isRemote: false,
        isActiveRemote: false,
      },
    ],
    sessionsUnavailableReason: null,
    connections: [],
    connectionsUnavailableReason: null,
    processes: [
      {
        pid: 4200,
        name: "msrdc.exe",
        executablePath: "C:\\Program Files\\WSL\\msrdc.exe",
        classification: "wslg_internal",
        label: "Cliente Remote Desktop interno do WSLg",
        explanation:
          "O WSL usa msrdc.exe localmente para integrar janelas gráficas Linux ao Windows. Não indica acesso externo.",
        externalConnections: 0,
        listeningPorts: [],
      },
    ],
    processesUnavailableReason: null,
    checks: [
      {
        id: "rdp_config",
        title: "Configuração e porta do RDP",
        status: "ok",
        summary: "RDP configurado para recusar conexões de entrada.",
        details: ["Entrada RDP: desabilitada (fDenyTSConnections = 1)"],
      },
      {
        id: "sessions",
        title: "Sessões do sistema",
        status: "ok",
        summary: "Nenhuma sessão remota registrada; apenas sessão local.",
        details: ["console (ID 1) · usuário: lucas · Active"],
      },
      {
        id: "network_rdp",
        title: "Conexões e portas de acesso remoto",
        status: "ok",
        summary: "Nenhuma conexão ou escuta ativa encontrada.",
        details: [],
      },
      {
        id: "processes",
        title: "Processos de acesso remoto e WSLg",
        status: "ok",
        summary: "Nenhum agente de terceiros; 1 componente(s) interno(s) do WSLg detectado(s).",
        details: [],
      },
    ],
    collectedAtUnixMs: 1_760_000_000_000,
    elevated: false,
  };
}

describe("SecurityHub", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("exibe diagnóstico WSLg interno sem falso alarme", async () => {
    mockedInvoke.mockResolvedValueOnce(makeWslgOnlyAudit());

    render(<SecurityHub />);

    await waitFor(() => {
      expect(
        screen.getByText("Sem indício de acesso remoto — apenas integração interna do WSLg")
      ).toBeInTheDocument();
    });

    expect(screen.getByText("Desabilitado")).toBeInTheDocument();
    expect(screen.getByText("porta padrão 3389")).toBeInTheDocument();
    expect(screen.getByText("WSLg interno")).toBeInTheDocument();
    expect(screen.getByText("C:\\Program Files\\WSL\\msrdc.exe")).toBeInTheDocument();
    expect(screen.getByText("Local · Active")).toBeInTheDocument();
  });

  it("sinaliza sessão remota ativa em porta RDP customizada", async () => {
    const activeAudit: RemoteAccessAudit = {
      ...makeWslgOnlyAudit(),
      verdict: "active_remote_session",
      verdictTitle: "Sessão remota ativa detectada agora",
      verdictSummary:
        "Encontrada evidência direta de acesso remoto em andamento na porta RDP 3390.",
      rdpConfig: {
        enabled: true,
        configuredPort: 3390,
        isCustomPort: true,
        nlaRequired: true,
        serviceStatus: "Em execução",
        servicePid: 900,
        unavailableReason: null,
      },
      sessions: [
        {
          sessionName: "rdp-tcp#0",
          username: "suporte",
          sessionId: 2,
          state: "Active",
          isRemote: true,
          isActiveRemote: true,
        },
      ],
      connections: [
        {
          protocol: "TCP",
          direction: "inbound_rdp",
          localAddress: "192.168.1.10",
          localPort: 3390,
          remoteAddress: "203.0.113.42",
          remotePort: 51234,
          isLoopback: false,
          isPublicBind: false,
          pid: 900,
          processName: "svchost.exe",
          executablePath: "C:\\Windows\\System32\\svchost.exe",
          reason: "Conexão estabelecida na porta RDP customizada (3390)",
        },
      ],
    };

    mockedInvoke.mockResolvedValueOnce(activeAudit);

    render(<SecurityHub />);

    await waitFor(() => {
      expect(screen.getByText("Sessão remota ativa detectada agora")).toBeInTheDocument();
    });

    expect(screen.getByText("Habilitado")).toBeInTheDocument();
    expect(screen.getByText("porta customizada 3390")).toBeInTheDocument();
    expect(screen.getByText("ESTABELECIDA")).toBeInTheDocument();
    expect(screen.getByText("Remota · Active")).toBeInTheDocument();
  });

  it("nunca exibe zero fictício quando uma fonte falha e permite reexecutar", async () => {
    const partialAudit: RemoteAccessAudit = {
      ...makeWslgOnlyAudit(),
      verdict: "partial",
      verdictTitle: "Verificação parcial — nem todos os sinais puderam ser lidos",
      sessions: [],
      sessionsUnavailableReason: "qwinsta indisponível neste Windows",
    };

    mockedInvoke.mockResolvedValueOnce(partialAudit);
    mockedInvoke.mockResolvedValueOnce(makeWslgOnlyAudit());

    const onNavigate = vi.fn();
    render(<SecurityHub onNavigate={onNavigate} />);

    await waitFor(() => {
      expect(screen.getByText("qwinsta indisponível neste Windows")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: /Verificar agora/i }));

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledTimes(2);
      expect(mockedInvoke).toHaveBeenCalledWith("run_remote_access_audit");
    });

    fireEvent.click(screen.getByRole("button", { name: "Revisar serviços" }));
    expect(onNavigate).toHaveBeenCalledWith("Serviços e atualizações");
  });
});
