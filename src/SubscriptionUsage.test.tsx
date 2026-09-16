import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { formatReset, formatUsd, hasMeasuredQuota, isProviderDisabled, safePercent, selectFlyoutQuotaWindows, shortQuotaWindowLabel, SubscriptionUsage, type SubscriptionUsageReport } from "./SubscriptionUsage";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const report: SubscriptionUsageReport = {
  providers: [
    {
      id: "claude", provider: "Claude Code", subtitle: "Assinatura Anthropic", status: "available", plan: "team",
      environment: "WSL · assinatura única", source: "Painel /usage do Claude Code (WSL)", sampledAtUnixMs: 1_725_000_000_000,
      windows: [
        { id: "session", label: "Sessão", usedPercent: 8, remainingPercent: 92, resetDescription: "Resets 21:00" },
        { id: "weekly", label: "Semanal", usedPercent: 16, remainingPercent: 84 },
      ],
    },
    {
      id: "codex", provider: "Codex", subtitle: "ChatGPT", status: "available", plan: "plus",
      environment: "Windows · assinatura única", source: "Codex app-server", sampledAtUnixMs: 1_725_000_000_000,
      creditsRemaining: 0, resetCreditsAvailable: 2,
      windows: [
        { id: "session", label: "Sessão", usedPercent: 0, remainingPercent: 100, resetsAtUnixSeconds: 4_000_000_000 },
        { id: "weekly", label: "Semanal", usedPercent: 45, remainingPercent: 55, resetsAtUnixSeconds: 4_000_100_000 },
        { id: "reserve-session", label: "GPT Reserve · sessão", usedPercent: 38, remainingPercent: 62 },
      ],
    },
    {
      id: "antigravity", provider: "Antigravity", subtitle: "Google AI Pro", status: "unavailable", source: "Painel /usage",
      unavailableReason: "A CLI aguarda confirmação humana de confiança no diretório.", windows: [],
    },
  ],
  sampledAtUnixMs: 1_725_000_000_000,
  durationMs: 14300,
  privacyNote: "Consultas locais e oficiais; credenciais nunca são retornadas à interface.",
};

describe("SubscriptionUsage", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
  });

  it("sanitizes percentages and formats reset times", () => {
    expect(safePercent(null)).toBeNull();
    expect(safePercent(Number.NaN)).toBeNull();
    expect(safePercent(110)).toBe(100);
    expect(formatReset(1_000, 1_000_000)).toBe("Leitura vencida");
    expect(formatReset(1_000 + 90 * 60, 1_000_000)).toBe("Reseta em 1h 30min");
  });

  it("renders measured providers and preserves a real zero balance", async () => {
    vi.mocked(invoke).mockResolvedValue(report as never);
    render(<SubscriptionUsage />);

    expect(await screen.findByText("Team")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("Codex")).toBeInTheDocument();
    expect(screen.queryByText("Antigravity")).not.toBeInTheDocument();
    expect(screen.getByText("92% restante")).toBeInTheDocument();
    expect(screen.getByText("55% restante")).toBeInTheDocument();
    expect(screen.getByText("0 créditos")).toBeInTheDocument();
    expect(screen.getByText("2 disponíveis")).toBeInTheDocument();
    expect(screen.getByText("2", { selector: ".quota-toolbar-actions b" })).toBeInTheDocument();
    expect(screen.getAllByRole("progressbar", { name: "Sessão: cota restante" })[0]).toHaveAttribute("aria-valuenow", "92");
    expect(screen.queryByText("0% restante")).not.toBeInTheDocument();
  });

  it("forces a provider refresh without hiding the current report", async () => {
    vi.mocked(invoke).mockResolvedValue(report as never);
    render(<SubscriptionUsage />);
    await screen.findByText("Team");
    fireEvent.click(screen.getByRole("button", { name: "ATUALIZAR COTAS" }));
    await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("get_subscription_usage", { forceRefresh: true }));
    expect(screen.getByText("Codex")).toBeInTheDocument();
  });

  it("hides providers without a confirmed quota instead of inventing a card", async () => {
    vi.mocked(invoke).mockResolvedValue(report as never);
    render(<SubscriptionUsage />);
    await screen.findByText("Claude Code");
    expect(screen.queryByText("Antigravity")).not.toBeInTheDocument();
    expect(screen.queryByText("Cota não confirmada")).not.toBeInTheDocument();
  });

  it("opens and closes the API key configuration modal without breaking layout", async () => {
    const reportWithOpenRouter: SubscriptionUsageReport = {
      ...report,
      providers: [
        ...report.providers,
        {
          id: "openrouter", provider: "OpenRouter", subtitle: "Saldo em Dólares (USD)", status: "available",
          source: "OpenRouter API", creditsRemaining: 0.2971, windows: [],
        },
      ],
    };
    vi.mocked(invoke).mockResolvedValue(reportWithOpenRouter as never);
    render(<SubscriptionUsage />);

    expect(await screen.findByText("CONFIGURAR OPENROUTER")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // Abrir via botão da toolbar
    fireEvent.click(screen.getByText("CONFIGURAR OPENROUTER"));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Configurar OpenRouter" })).toBeInTheDocument();

    // Fechar modal
    fireEvent.click(screen.getByText("CANCELAR"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // Abrir via botão no card do provedor
    fireEvent.click(screen.getByRole("button", { name: "Configurar chave OpenRouter" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("keeps OpenCode Go discoverable and reachable for key setup even without a confirmed quota", async () => {
    const reportWithOpencodeGo: SubscriptionUsageReport = {
      ...report,
      providers: [
        ...report.providers,
        {
          id: "opencode-go", provider: "OpenCode Go", subtitle: "Assinatura de modelos Open (USD)", status: "unavailable",
          source: "OpenCode Go API · /zen/go/v1/usage", windows: [],
          unavailableReason: "Chave não configurada. Informe uma chave nas configurações de Cotas de IA para consultar a cota do OpenCode Go.",
        },
      ],
    };
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_subscription_settings") {
        return Promise.resolve({ openrouterConfigured: true, opencodeGoConfigured: false }) as never;
      }
      return Promise.resolve(reportWithOpencodeGo) as never;
    });
    render(<SubscriptionUsage />);

    expect(await screen.findByText("OpenCode Go")).toBeInTheDocument();
    expect(screen.getByText("Cota não confirmada")).toBeInTheDocument();
    expect(screen.getByText("Chave não configurada. Informe uma chave nas configurações de Cotas de IA para consultar a cota do OpenCode Go.")).toBeInTheDocument();

    // Abrir via botão do card indisponível
    fireEvent.click(screen.getByRole("button", { name: "CONFIGURAR CHAVE OPENCODE GO" }));
    expect(screen.getByRole("heading", { name: "Configurar OpenCode Go" })).toBeInTheDocument();
    fireEvent.click(screen.getByText("CANCELAR"));

    // Abrir via botão dedicado da toolbar
    fireEvent.click(screen.getByRole("button", { name: "CONFIGURAR OPENCODE GO" }));
    expect(screen.getByRole("heading", { name: "Configurar OpenCode Go" })).toBeInTheDocument();
  });

  it("formats OpenRouter values explicitly as dollars", () => {
    expect(formatUsd(0.2971)).toBe("$ 0,2971");
    expect(formatUsd(0)).toBe("$ 0,00");
    expect(hasMeasuredQuota({ ...report.providers[2] })).toBe(false);
  });

  it("uses one global loading state instead of card skeletons", () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_subscription_usage" || command === "get_subscription_settings") {
        return new Promise(() => {}) as never;
      }
      return Promise.resolve() as never;
    });
    const { container } = render(<SubscriptionUsage />);

    expect(screen.getByRole("status", { name: "Consultando cotas locais" })).toBeInTheDocument();
    expect(container.querySelector(".quota-skeleton")).not.toBeInTheDocument();
  });

  it("labels tray windows by their real duration instead of truncating model names", () => {
    expect(shortQuotaWindowLabel({ id: "agy-0", label: "Gemini Models · 5 horas", windowMinutes: 300 })).toBe("5h");
    expect(shortQuotaWindowLabel({ id: "agy-1", label: "Gemini Models · Semanal", windowMinutes: 10080 })).toBe("7d");
    expect(shortQuotaWindowLabel({ id: "agy-2", label: "Mensal", windowMinutes: 43200 })).toBe("30d");
    expect(shortQuotaWindowLabel({ id: "session", label: "Sessão atual" })).toBe("5h");
    expect(shortQuotaWindowLabel({ id: "weekly", label: "Cota semanal" })).toBe("7d");
    expect(shortQuotaWindowLabel({ id: "custom", label: "Reserva extra" })).toBe("Rese…");
  });

  it("picks the worst measured window of each duration for the tray", () => {
    const windows = [
      { id: "agy-0", label: "Gemini Models · Semanal", remainingPercent: 85, windowMinutes: 10080 },
      { id: "agy-1", label: "Gemini Models · 5 horas", remainingPercent: 100, windowMinutes: 300 },
      { id: "agy-2", label: "Claude and GPT models · Semanal", remainingPercent: 66, windowMinutes: 10080 },
      { id: "agy-3", label: "Claude and GPT models · 5 horas", remainingPercent: 100, windowMinutes: 300 },
    ];
    const picked = selectFlyoutQuotaWindows(windows);
    expect(picked).toHaveLength(2);
    expect(picked[0].label).toBe("Gemini Models · 5 horas");
    expect(picked[1].label).toBe("Claude and GPT models · Semanal");
    expect(selectFlyoutQuotaWindows([
      ...windows,
      { id: "vencida", label: "Claude and GPT models · 5 horas", remainingPercent: null, windowMinutes: 300, unavailableReason: "A janela foi reiniciada." },
    ])).toHaveLength(2);
  });

  it("renders a disabled provider as inactive and re-enables it on demand", async () => {
    const disabledClaude = { ...report.providers[0], disabled: true };
    const settings = { openrouterConfigured: false, opencodeGoConfigured: false, disabledProviders: ["claude"] };
    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === "get_subscription_settings") return Promise.resolve(settings) as never;
      if (command === "set_provider_enabled") {
        expect(args).toEqual({ providerId: "claude", enabled: true });
        return Promise.resolve({ ...settings, disabledProviders: [] }) as never;
      }
      return Promise.resolve({ ...report, providers: [disabledClaude, report.providers[1]] }) as never;
    });

    const { container } = render(<SubscriptionUsage />);
    expect(await screen.findByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("DESATIVADO")).toBeInTheDocument();
    expect(screen.getByText("Provedor desativado")).toBeInTheDocument();
    expect(isProviderDisabled(disabledClaude)).toBe(true);

    const claudeCard = container.querySelector(".quota-provider-card.provider-claude");
    expect(claudeCard).toHaveClass("is-disabled");
    expect(claudeCard?.querySelector(".quota-track")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Reativar Claude Code" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_provider_enabled", { providerId: "claude", enabled: true }));
    await waitFor(() => expect(screen.queryByText("DESATIVADO")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Desativar Claude Code" })).toBeInTheDocument();
    expect(screen.getByText("92% restante")).toBeInTheDocument();
  });
});
