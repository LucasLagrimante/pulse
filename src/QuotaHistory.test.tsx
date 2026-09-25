import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
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

describe("QuotaHistoryChart", () => {
  beforeEach(() => {
    vi.clearAllMocks();
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

  const history = [
    point(day2026(8, 10, 9), 20),
    point(day2026(8, 10, 18), 40),
    point(day2026(8, 11, 10), 80),
    point(day2026(8, 12, 10), 35),
  ];

  it("exibe máximo, média e dia de maior consumo dos últimos 30 dias", async () => {
    vi.mocked(invoke).mockResolvedValue({ points: history, retainedDays: 35 } as never);
    render(
      <QuotaHistoryChart providerId="claude" windowId="session" windowLabel="Sessão" mode="percent" />,
    );
    expect(await screen.findByText("80%")).toBeInTheDocument();
    expect(screen.getByText("MÉDIA")).toBeInTheDocument();
    expect(screen.getByText("44%")).toBeInTheDocument();
    expect(screen.getByText("DIA DO MÁXIMO")).toBeInTheDocument();
    expect(screen.getByText("11/09")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("get_quota_history", {
      providerId: "claude",
      windowId: "session",
      days: 30,
    });
  });

  it("mostra tooltip com valor e data ao passar o mouse", async () => {
    vi.mocked(invoke).mockResolvedValue({ points: history, retainedDays: 35 } as never);
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

  it("informa quando ainda não há histórico registrado", async () => {
    vi.mocked(invoke).mockResolvedValue({ points: [], retainedDays: 35 } as never);
    render(
      <QuotaHistoryChart providerId="codex" windowId="weekly" windowLabel="Semanal" mode="percent" />,
    );
    expect(await screen.findByText(/Sem histórico ainda/)).toBeInTheDocument();
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
