import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearQuotaHistoryCache,
  computeCycleInfo,
  downsampleValues,
  groupValuesByDay,
  historyPointValue,
  peakBucket,
  QuotaHistoryButton,
  QuotaHistoryChart,
  type QuotaHistoryPoint,
  type TimedValue,
} from "./QuotaHistory";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

function point(at: number, usedPercent: number | null): QuotaHistoryPoint {
  return {
    providerId: "claude",
    windowId: "session",
    label: "Sessão",
    usedPercent,
    remainingPercent: usedPercent == null ? null : 100 - usedPercent,
    creditsRemaining: null,
    sampledAtUnixMs: at,
  };
}

function day2026(monthIndex: number, day: number, hour = 12): number {
  return new Date(2026, monthIndex, day, hour).getTime();
}

describe("historyPointValue", () => {
  it("prefere o percentual usado e nunca inventa zero", () => {
    expect(historyPointValue(point(1, 42), "percent")).toBe(42);
    expect(historyPointValue(point(1, null), "percent")).toBeNull();
    expect(
      historyPointValue(
        { ...point(1, null), remainingPercent: 70 },
        "percent",
      ),
    ).toBe(30);
    expect(
      historyPointValue(
        { ...point(1, null), creditsRemaining: 12.5 },
        "credits",
      ),
    ).toBe(12.5);
    expect(historyPointValue(point(1, Number.NaN), "percent")).toBeNull();
  });
});

describe("groupValuesByDay", () => {
  it("agrega máximo e média por dia e elege o pico (empate: mais recente)", () => {
    const values: TimedValue[] = [
      { at: day2026(8, 10, 9), value: 20 },
      { at: day2026(8, 10, 18), value: 60 },
      { at: day2026(8, 11, 10), value: 60 },
      { at: day2026(8, 12, 10), value: 35 },
    ];
    const buckets = groupValuesByDay(values);
    expect(buckets).toHaveLength(3);
    expect(buckets[0]).toMatchObject({ label: "10/09", max: 60, avg: 40, count: 2 });
    expect(peakBucket(buckets)?.label).toBe("11/09");
    expect(peakBucket([])).toBeNull();
  });
});

describe("downsampleValues", () => {
  it("preserva primeira e última amostra ao reduzir", () => {
    const values: TimedValue[] = Array.from({ length: 200 }, (_, i) => ({
      at: i,
      value: i,
    }));
    const sampled = downsampleValues(values, 90);
    expect(sampled).toHaveLength(90);
    expect(sampled[0]).toEqual({ at: 0, value: 0 });
    expect(sampled[sampled.length - 1]).toEqual({ at: 199, value: 199 });
    expect(downsampleValues(values.slice(0, 5), 90)).toHaveLength(5);
  });
});

describe("computeCycleInfo", () => {
  it("calcula ciclo semanal de 7 dias ancorado no reset futuro", () => {
    const now = 1_790_000_000_000;
    const resetsAt = Math.floor(now / 1000) + 2 * 86400; // Daqui a 2 dias
    const cycle = computeCycleInfo(
      { windowMinutes: 10080, resetsAtUnixSeconds: resetsAt, label: "Semanal" },
      "percent",
      now,
    );
    expect(cycle.popoverBadge).toBe("CICLO SEMANAL (7 DIAS)");
    expect(cycle.cycleLabel).toBe("ciclo semanal");
    expect(cycle.cycleStartMs).toBe(resetsAt * 1000 - 10080 * 60_000);
  });

  it("reinicia ciclo quando resetsAt já passou", () => {
    const now = 1_790_000_000_000;
    const resetsAt = Math.floor(now / 1000) - 3600; // 1 hora atrás
    const cycle = computeCycleInfo(
      { windowMinutes: 10080, resetsAtUnixSeconds: resetsAt, label: "Semanal" },
      "percent",
      now,
    );
    expect(cycle.popoverBadge).toBe("CICLO SEMANAL (RENOVADO)");
    expect(cycle.cycleStartMs).toBe(resetsAt * 1000);
  });

  it("calcula ciclo mensal ancorado no reset", () => {
    const now = 1_790_000_000_000;
    const resetsAt = Math.floor(now / 1000) + 15 * 86400;
    const cycle = computeCycleInfo(
      { windowMinutes: 43200, resetsAtUnixSeconds: resetsAt, label: "Mensal" },
      "percent",
      now,
    );
    expect(cycle.popoverBadge).toBe("CICLO MENSAL");
    expect(cycle.cycleLabel).toBe("ciclo mensal");
  });

  it("calcula janela de 5 horas ancorada no reset", () => {
    const now = 1_790_000_000_000;
    const resetsAt = Math.floor(now / 1000) + 7200; // Daqui a 2 horas
    const cycle = computeCycleInfo(
      { windowMinutes: 300, resetsAtUnixSeconds: resetsAt, label: "Sessão" },
      "percent",
      now,
    );
    expect(cycle.popoverBadge).toBe("JANELA ATUAL (5H)");
    expect(cycle.cycleLabel).toBe("janela atual (5h)");
    expect(cycle.cycleStartMs).toBe(resetsAt * 1000 - 300 * 60_000);
  });

  it("mantém 30 dias para créditos", () => {
    const cycle = computeCycleInfo(null, "credits", 1_790_000_000_000);
    expect(cycle.popoverBadge).toBe("ÚLTIMOS 30 DIAS");
  });
});

describe("QuotaHistoryChart", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clearQuotaHistoryCache();
    Element.prototype.getBoundingClientRect = vi.fn(() => ({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 320,
      bottom: 118,
      width: 320,
      height: 118,
      toJSON: () => {},
    }));
  });

  afterEach(() => {
    clearQuotaHistoryCache();
  });

  const createHistory = () => {
    const now = Date.now();
    const oneDay = 24 * 3600 * 1000;
    const peakTime = now - 2 * oneDay;
    return {
      points: [
        point(now - 3 * oneDay, 20),
        point(now - 3 * oneDay + 3600 * 1000, 40),
        point(peakTime, 80),
        point(now - oneDay, 35),
      ],
      peakDateLabel: new Date(peakTime).toLocaleDateString("pt-BR", { day: "2-digit", month: "2-digit" }),
    };
  };

  it("exibe máximo, média e dia de maior consumo do ciclo", async () => {
    const { points, peakDateLabel } = createHistory();
    vi.mocked(invoke).mockResolvedValue({ points, retainedDays: 35 } as never);
    render(
      <QuotaHistoryChart providerId="claude" windowId="session" windowLabel="Sessão" mode="percent" />,
    );
    expect(await screen.findByText("80%")).toBeInTheDocument();
    expect(screen.getByText("MÉDIA")).toBeInTheDocument();
    expect(screen.getByText("44%")).toBeInTheDocument();
    expect(screen.getByText("DIA DO MÁXIMO")).toBeInTheDocument();
    expect(screen.getByText(peakDateLabel)).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("get_quota_history", {
      providerId: "claude",
      windowId: "session",
      days: 30,
      sinceUnixMs: expect.any(Number),
    });
  });

  it("mostra tooltip com valor e data ao passar o mouse", async () => {
    const { points } = createHistory();
    vi.mocked(invoke).mockResolvedValue({ points, retainedDays: 35 } as never);
    const { container } = render(
      <QuotaHistoryChart providerId="claude" windowId="session" windowLabel="Sessão" mode="percent" />,
    );
    await screen.findByText("80%");
    const plot = container.querySelector(".quota-history-plot") as HTMLElement;
    fireEvent.mouseMove(plot, { clientX: 200 });
    await waitFor(() => {
      expect(container.querySelector(".quota-history-tooltip")).not.toBeNull();
    });
  });

  it("informa quando ainda não há histórico registrado no ciclo", async () => {
    vi.mocked(invoke).mockResolvedValue({ points: [], retainedDays: 35 } as never);
    render(
      <QuotaHistoryChart providerId="codex" windowId="weekly" windowLabel="Semanal" mode="percent" />,
    );
    expect(await screen.findByText(/Sem histórico ainda no ciclo atual/)).toBeInTheDocument();
  });

  it("exibe horários no eixo X quando o intervalo é de até 24 horas", async () => {
    const now = Date.now();
    const start = now - 4 * 3600 * 1000;
    const end = now;
    const resetsAt = Math.floor((now + 3600 * 1000) / 1000); // 1h no futuro
    const shortHistory = [
      point(start, 10),
      point(end, 50),
    ];
    vi.mocked(invoke).mockResolvedValue({ points: shortHistory, retainedDays: 35 } as never);
    render(
      <QuotaHistoryChart
        providerId="claude"
        windowId="session"
        windowLabel="Sessão"
        mode="percent"
        quota={{ windowMinutes: 300, resetsAtUnixSeconds: resetsAt }}
      />,
    );
    const startHour = new Date(start).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
    const endHour = new Date(end).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
    expect(await screen.findByText(startHour)).toBeInTheDocument();
    expect(screen.getByText(endHour)).toBeInTheDocument();
  });
});

describe("QuotaHistoryButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("abre o gráfico da janela ao clicar", async () => {
    vi.mocked(invoke).mockResolvedValue({ points: [], retainedDays: 35 } as never);
    render(
      <QuotaHistoryButton providerId="claude" windowId="session" windowLabel="Sessão" mode="percent" />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Ver histórico de uso de Sessão/ }));
    expect(await screen.findByRole("dialog", { name: "Histórico de uso — Sessão" })).toBeInTheDocument();
  });

  it("vira para a esquerda quando não há espaço à direita", async () => {
    vi.mocked(invoke).mockResolvedValue({ points: [], retainedDays: 35 } as never);
    Element.prototype.getBoundingClientRect = vi.fn(() => ({
      x: 900,
      y: 100,
      left: 900,
      top: 100,
      right: 922,
      bottom: 122,
      width: 22,
      height: 22,
      toJSON: () => {},
    }));
    render(
      <QuotaHistoryButton providerId="claude" windowId="session" windowLabel="Sessão" mode="percent" />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Ver histórico de uso de Sessão/ }));
    const dialog = await screen.findByRole("dialog", { name: "Histórico de uso — Sessão" });
    expect(dialog.className).toContain("is-down-right");
  });

  it("abre para baixo e para a direita com espaço livre", async () => {
    vi.mocked(invoke).mockResolvedValue({ points: [], retainedDays: 35 } as never);
    Element.prototype.getBoundingClientRect = vi.fn(() => ({
      x: 80,
      y: 100,
      left: 80,
      top: 100,
      right: 102,
      bottom: 122,
      width: 22,
      height: 22,
      toJSON: () => {},
    }));
    render(
      <QuotaHistoryButton providerId="claude" windowId="session" windowLabel="Sessão" mode="percent" />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Ver histórico de uso de Sessão/ }));
    const dialog = await screen.findByRole("dialog", { name: "Histórico de uso — Sessão" });
    expect(dialog.className).toContain("is-down-left");
  });
});
