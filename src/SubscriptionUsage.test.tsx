import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { formatReset, formatUsd, hasMeasuredQuota, safePercent, SubscriptionUsage, type SubscriptionUsageReport } from "./SubscriptionUsage";

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
});
