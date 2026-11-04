import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { formatCountdown, groupQuotaWindowsByModel, shortModelName } from "./subscriptionHelpers";
import { formatReset, formatUsd, hasMeasuredQuota, isProviderDisabled, isWindowChartEligible, reorderIds, safePercent, selectFlyoutQuotaWindows, shortQuotaWindowLabel, SubscriptionUsage, type SubscriptionUsageReport } from "./SubscriptionUsage";

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
    expect(formatReset(1_000, 1_000_000)).toBe("Ciclo renovado");
    expect(formatReset(1_000 + 90 * 60, 1_000_000)).toBe("Reseta em 1h 30min");
  });

  it("determines chart eligibility correctly for measured windows", () => {
    expect(isWindowChartEligible({ id: "rolling-0", label: "Janela atual (5h)", remainingPercent: 90 })).toBe(true);
    expect(isWindowChartEligible({ id: "agy-1", label: "Gemini Models · 5 horas", remainingPercent: 100 })).toBe(true);
    expect(isWindowChartEligible({ id: "session", label: "Sessão", remainingPercent: 50 })).toBe(true);
    expect(isWindowChartEligible({ id: "weekly", label: "Semanal", remainingPercent: 80 })).toBe(true);
    expect(isWindowChartEligible({ id: "unavail", label: "Indisponível", remainingPercent: null, unavailableReason: "Sem leitura" })).toBe(false);
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

  it("reorders ids by drag position and rejects invalid moves", () => {
    expect(reorderIds(["claude", "codex", "openrouter"], "openrouter", "claude")).toEqual(["openrouter", "claude", "codex"]);
    expect(reorderIds(["claude", "codex"], "claude", "claude")).toBeNull();
    expect(reorderIds(["claude"], "desconhecido", "claude")).toBeNull();
  });

  it("orders active providers by preference and pushes disabled ones to the end", async () => {
    const providers = [
      { ...report.providers[0] },
      { ...report.providers[1] },
      { id: "openrouter", provider: "OpenRouter", subtitle: "Saldo em Dólares (USD)", status: "available", source: "OpenRouter API", creditsRemaining: 4.6856, windows: [] },
      { id: "opencode-go", provider: "OpenCode Go", subtitle: "Assinatura de modelos Open (USD)", status: "available", source: "OpenCode Go API", creditsRemaining: 1, windows: [], disabled: true },
    ];
    const settings = {
      openrouterConfigured: true,
      opencodeGoConfigured: true,
      disabledProviders: ["opencode-go"],
      providerOrder: ["openrouter", "codex", "claude", "opencode-go"],
    };
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_subscription_settings") return Promise.resolve(settings) as never;
      return Promise.resolve({ ...report, providers }) as never;
    });

    const { container } = render(<SubscriptionUsage />);
    await screen.findByText("OpenRouter");
    // A ordem só vale depois que get_subscription_settings responde; em máquina
    // lenta (CI) os cards aparecem antes na ordem do relatório.
    const cardOrder = () =>
      [...container.querySelectorAll(".quota-provider-card")].map(card =>
        [...card.classList].find(className => className.startsWith("provider-"))?.replace("provider-", ""),
      );
    await waitFor(() => expect(cardOrder()).toEqual(["openrouter", "codex", "claude", "opencode-go"]));
  });

  it("persists a new order when a card is dropped on another", async () => {
    const providers = [
      { ...report.providers[0] },
      { ...report.providers[1] },
      { id: "openrouter", provider: "OpenRouter", subtitle: "Saldo em Dólares (USD)", status: "available", source: "OpenRouter API", creditsRemaining: 4.6856, windows: [] },
    ];
    const settings = { openrouterConfigured: true, opencodeGoConfigured: true, providerOrder: ["claude", "codex", "openrouter"] };
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_subscription_settings") return Promise.resolve(settings) as never;
      if (command === "set_provider_order") {
        return Promise.resolve({ ...settings, providerOrder: ["codex", "claude", "openrouter"] }) as never;
      }
      return Promise.resolve({ ...report, providers }) as never;
    });

    const { container } = render(<SubscriptionUsage />);
    await screen.findByText("OpenRouter");
    await waitFor(() => expect(container.querySelectorAll(".quota-provider-card")).toHaveLength(3));

    const cards = container.querySelectorAll(".quota-provider-card");
    const dataTransfer = { dropEffect: "", effectAllowed: "", setData: vi.fn(), getData: vi.fn() };
    fireEvent.dragStart(cards[0], { dataTransfer });
    fireEvent.dragOver(cards[1], { dataTransfer });
    fireEvent.drop(cards[1], { dataTransfer });

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_provider_order", { order: ["codex", "claude", "openrouter"] }));
  });

  it("groups per-model quota windows into a model × window matrix", () => {
    const matrix = groupQuotaWindowsByModel([
      { id: "agy-0", label: "Gemini Models · Semanal", remainingPercent: 92, windowMinutes: 10080 },
      { id: "agy-1", label: "Gemini Models · 5 horas", remainingPercent: 97, windowMinutes: 300 },
      { id: "agy-2", label: "Claude and GPT models · Semanal", remainingPercent: 100, windowMinutes: 10080 },
      { id: "agy-4", label: "Gemini 4 Argon · Semanal", remainingPercent: 100, windowMinutes: 10080 },
    ]);
    expect(matrix?.columns).toEqual([300, 10080]);
    expect(matrix?.rows.map(row => row.model)).toEqual(["Gemini Models", "Claude and GPT models", "Gemini 4 Argon"]);
    expect(matrix?.rows[2].cells.has(300)).toBe(false);
    // Codex/Claude (sem "Modelo · Janela") continuam na lista simples.
    expect(groupQuotaWindowsByModel(report.providers[1].windows)).toBeNull();
    expect(shortModelName("Claude and GPT models")).toBe("Claude + GPT");
    expect(shortModelName("Gemini Models")).toBe("Gemini");
    expect(shortModelName("Gemini 4 Argon")).toBe("Gemini 4 Argon");
  });

  it("formats a compact countdown until the window renews", () => {
    expect(formatCountdown(null)).toBeNull();
    expect(formatCountdown(1_000, 2_000_000)).toBe("renovado");
    expect(formatCountdown(1_000 + 45 * 60, 1_000_000)).toBe("45min");
    expect(formatCountdown(1_000 + 2 * 3600 + 5 * 60, 1_000_000)).toBe("2h05");
    expect(formatCountdown(1_000 + 4 * 86400 + 2 * 3600, 1_000_000)).toBe("4d2h");
    expect(formatCountdown(1_000 + 7 * 86400, 1_000_000)).toBe("7d");
  });

  it("renders Antigravity as a compact matrix with an explicit gap for missing windows", async () => {
    const future = Math.floor(Date.now() / 1000) + 3 * 3600 + 53 * 60;
    const matrixReport: SubscriptionUsageReport = {
      ...report,
      providers: [
        {
          id: "antigravity", provider: "Antigravity", subtitle: "Google AI Pro", status: "available", source: "Antigravity CLI · /usage",
          windows: [
            { id: "agy-0", label: "Gemini Models · Semanal", usedPercent: 8, remainingPercent: 92, windowMinutes: 10080, resetsAtUnixSeconds: future + 86400 },
            { id: "agy-1", label: "Gemini Models · 5 horas", usedPercent: 3, remainingPercent: 97, windowMinutes: 300, resetsAtUnixSeconds: future },
            { id: "agy-2", label: "Claude and GPT models · Semanal", usedPercent: 0, remainingPercent: 100, windowMinutes: 10080, resetsAtUnixSeconds: future },
            { id: "agy-3", label: "Claude and GPT models · 5 horas", usedPercent: 0, remainingPercent: 100, windowMinutes: 300, resetsAtUnixSeconds: future },
            { id: "agy-4", label: "Gemini 4 Argon · Semanal", usedPercent: 19, remainingPercent: 81, windowMinutes: 10080, resetsAtUnixSeconds: future },
          ],
        },
      ],
    };
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_subscription_settings") {
        return Promise.resolve({ openrouterConfigured: false, opencodeGoConfigured: false }) as never;
      }
      return Promise.resolve(matrixReport) as never;
    });

    render(<SubscriptionUsage />);
    const table = await screen.findByRole("table", { name: "Cotas por modelo e janela" });
    expect(table.querySelectorAll('[role="row"]')).toHaveLength(4);
    expect(screen.getByRole("rowheader", { name: "Claude + GPT" })).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: "Gemini 4 Argon" })).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Gemini 4 Argon · Semanal: cota restante" })).toHaveAttribute("aria-valuenow", "81");
    expect(screen.getByLabelText("Gemini 4 Argon não tem janela 5 horas")).toHaveTextContent("—");
    expect(screen.getAllByText("↻ 3h53").length).toBeGreaterThan(0);
    expect(screen.queryByText("0%")).not.toBeInTheDocument();
  });

  it("includes the history chart button for both 5-hour and weekly windows", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_subscription_settings") {
        return Promise.resolve({ openrouterConfigured: false, opencodeGoConfigured: false }) as never;
      }
      return Promise.resolve(report) as never;
    });

    render(<SubscriptionUsage />);
    await screen.findByText("Claude Code");

    // Tanto "Sessão" (5h) quanto "Semanal" (7d) devem ter botão de histórico para os provedores medidos
    expect(screen.getAllByRole("button", { name: /Ver histórico de uso de Sessão/ })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: /Ver histórico de uso de Semanal/ })).toHaveLength(2);
  });

  it("resets quota meter to 100% and displays Ciclo renovado when the period has passed", async () => {
    const expiredReport: SubscriptionUsageReport = {
      ...report,
      providers: [
        {
          id: "opencode-go",
          provider: "OpenCode Go",
          subtitle: "Assinatura",
          status: "available",
          source: "API",
          windows: [
            {
              id: "weekly-1",
              label: "Semanal",
              usedPercent: 75,
              remainingPercent: 25,
              windowMinutes: 10080,
              resetsAtUnixSeconds: 1_000, // No passado
            },
          ],
        },
      ],
    };
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_subscription_settings") {
        return Promise.resolve({ openrouterConfigured: false, opencodeGoConfigured: false }) as never;
      }
      return Promise.resolve(expiredReport) as never;
    });

    render(<SubscriptionUsage />);
    await screen.findByText("OpenCode Go");

    expect(screen.getByText("100% restante")).toBeInTheDocument();
    expect(screen.getByText("0% usado")).toBeInTheDocument();
    expect(screen.getByText("Ciclo renovado")).toBeInTheDocument();
  });
});
