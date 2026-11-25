import { describe, expect, it } from "vitest";
import {
  computeCreditsBurnEstimate,
  computeQuotaPace,
  computeRecentHistoryPace,
  formatCompactDuration,
  formatExhaustedResetSummary,
  formatMatrixPaceHint,
  formatMeterPaceLine,
  formatPaceDuration,
  formatPaceRate,
  formatResetDateTime,
  projectPaceAtRecentRate,
  resolveWindowMinutes,
} from "./quotaPace";

const HOUR_MS = 3_600_000;
const DAY_MS = 86_400_000;

describe("computeQuotaPace", () => {
  const now = new Date(2026, 10, 24, 1, 30, 0).getTime(); // 24/11/2026 01:30

  it("calcula ritmo mensal do OpenCode Go (37% usados com 17d 15h restantes → no limite com sobra ~10%)", () => {
    const remainingMs = 17 * DAY_MS + 15 * HOUR_MS;
    const resetsAtUnixSeconds = Math.floor((now + remainingMs) / 1000);
    const pace = computeQuotaPace(
      {
        id: "monthly-2",
        label: "Mensal",
        usedPercent: 37,
        remainingPercent: 63,
        windowMinutes: 43200,
        resetsAtUnixSeconds,
      },
      now,
    );

    expect(pace?.status).toBe("tight");
    expect(pace?.rateUnit).toBe("day");
    expect(pace?.ratePerUnit).toBeCloseTo(37 / 12.375, 2);
    expect(pace?.projectedEndPercent).toBeCloseTo(89.7, 0);
    expect(pace?.projectedRemainingPercent).toBeCloseTo(10.3, 0);
    expect(pace?.depleteAtUnixMs).toBeNull();

    const line = formatMeterPaceLine(pace!);
    expect(line?.tone).toBe("warning");
    expect(line?.rateText).toBe("~3,0%/dia");
    expect(line?.verdictText).toContain("sobra ~10%");
  });

  it("calcula ritmo semanal folgado (8% usados em ~1,2d → dura até o reset)", () => {
    const remainingMs = 5 * DAY_MS + 19 * HOUR_MS;
    const pace = computeQuotaPace(
      {
        id: "weekly-1",
        label: "Semanal",
        usedPercent: 8,
        remainingPercent: 92,
        windowMinutes: 10080,
        resetsAtUnixSeconds: Math.floor((now + remainingMs) / 1000),
      },
      now,
    );

    expect(pace?.status).toBe("ok");
    expect(pace?.rateUnit).toBe("day");
    expect(pace?.projectedRemainingPercent).toBeGreaterThan(50);

    const line = formatMeterPaceLine(pace!);
    expect(line?.tone).toBe("ok");
    expect(line?.verdictText).toContain("dura até o reset");
  });

  it("detecta quando vai esgotar antes do reset (50% usados em 1,5d na janela semanal)", () => {
    const remainingMs = 5.5 * DAY_MS;
    const pace = computeQuotaPace(
      {
        id: "agy-2",
        label: "Claude and GPT models · Semanal",
        usedPercent: 50,
        remainingPercent: 50,
        windowMinutes: 10080,
        resetsAtUnixSeconds: Math.floor((now + remainingMs) / 1000),
      },
      now,
    );

    expect(pace?.status).toBe("will-deplete");
    expect(pace?.rateUnit).toBe("day");
    expect(pace?.ratePerUnit).toBeCloseTo(33.33, 1);
    expect(pace?.timeToDepleteMs).toBeCloseTo(1.5 * DAY_MS, -3);
    expect(pace?.depleteAtUnixMs).toBeCloseTo(now + 1.5 * DAY_MS, -3);
    expect(pace?.marginMs).toBeLessThan(0);

    const line = formatMeterPaceLine(pace!);
    expect(line?.tone).toBe("danger");
    expect(line?.rateText).toBe("~33%/dia");
    expect(line?.verdictText).toBe("esgota em ~1d 12h");
  });

  it("calcula janela de 5 horas em %/h", () => {
    const remainingMs = 92 * 60_000; // 1h32 restantes → 3h28 decorridas
    const pace = computeQuotaPace(
      {
        id: "agy-1",
        label: "Gemini Models · 5 horas",
        usedPercent: 62,
        remainingPercent: 38,
        windowMinutes: 300,
        resetsAtUnixSeconds: Math.floor((now + remainingMs) / 1000),
      },
      now,
    );

    expect(pace?.rateUnit).toBe("hour");
    expect(pace?.status).toBe("tight");
    expect(pace?.ratePerUnit).toBeCloseTo(62 / (208 / 60), 1);

    const hint = formatMatrixPaceHint(pace, { resetsAtUnixSeconds: Math.floor((now + remainingMs) / 1000) }, now);
    expect(hint?.tone).toBe("warning");
    expect(hint?.text).toContain("%/h");
  });

  it("marca como exhausted e destaca quando a cota volta mesmo sem histórico", () => {
    const resetsAtUnixSeconds = Math.floor(new Date(2026, 10, 25, 8, 0, 0).getTime() / 1000);
    const pace = computeQuotaPace(
      {
        id: "weekly",
        label: "Semanal",
        usedPercent: 100,
        remainingPercent: 0,
        windowMinutes: 10080,
        resetsAtUnixSeconds,
        resetDescription: "resets Nov 25, 8am (America/Sao_Paulo)",
      },
      now,
    );

    expect(pace?.status).toBe("exhausted");
    expect(pace?.remainingPercent).toBe(0);
    const summary = formatExhaustedResetSummary({ resetsAtUnixSeconds }, now);
    expect(summary).toContain("Renova em 1d 6h");
    expect(summary).toContain("amanhã, 08:00");
  });

  it("não extrapola no início do ciclo nem em mês de 31 dias recém-iniciado", () => {
    // 5 minutos decorridos numa janela de 5h (abaixo do piso de 15 min)
    const freshFiveHours = computeQuotaPace(
      {
        id: "rolling-0",
        label: "Janela atual (5h)",
        usedPercent: 5,
        remainingPercent: 95,
        windowMinutes: 300,
        resetsAtUnixSeconds: Math.floor((now + 295 * 60_000) / 1000),
      },
      now,
    );
    expect(freshFiveHours?.status).toBe("insufficient");
    expect(freshFiveHours?.ratePerUnit).toBeNull();

    // Mês de 31 dias: faltam 30,5 dias (maior que 43200 min = 30d)
    const month31 = computeQuotaPace(
      {
        id: "monthly-2",
        label: "Mensal",
        usedPercent: 2,
        remainingPercent: 98,
        windowMinutes: 43200,
        resetsAtUnixSeconds: Math.floor((now + 30.5 * DAY_MS) / 1000),
      },
      now,
    );
    expect(month31?.status).toBe("insufficient");
    expect(month31?.elapsedMs).toBe(0);
  });

  it("retorna idle quando o consumo é zero e no-reset quando não há horário de reset", () => {
    const idle = computeQuotaPace(
      {
        id: "weekly",
        label: "Semanal",
        usedPercent: 0,
        remainingPercent: 100,
        windowMinutes: 10080,
        resetsAtUnixSeconds: Math.floor((now + 3 * DAY_MS) / 1000),
      },
      now,
    );
    expect(idle?.status).toBe("idle");

    const noReset = computeQuotaPace(
      { id: "session", label: "Sessão", usedPercent: 25, remainingPercent: 75 },
      now,
    );
    expect(noReset?.status).toBe("no-reset");
  });

  it("trata cota esgotada sem timestamp nem descrição sem inventar ritmo nem horário", () => {
    const pace = computeQuotaPace(
      {
        id: "weekly",
        label: "Semanal",
        usedPercent: 100,
        remainingPercent: 0,
        windowMinutes: 10080,
        resetsAtUnixSeconds: null,
        resetDescription: null,
      },
      now,
    );
    expect(pace?.status).toBe("exhausted");
    expect(pace?.ratePerUnit).toBeNull();
    expect(pace?.resetsAtMs).toBeNull();
    expect(formatMeterPaceLine(pace!)).toBeNull();
    expect(formatExhaustedResetSummary({ resetsAtUnixSeconds: null, resetDescription: null }, now)).toBe(
      "Horário de reset não informado",
    );
    expect(
      formatExhaustedResetSummary({ resetsAtUnixSeconds: 0, resetDescription: "Resets Monday 00:00 UTC" }, now),
    ).toBe("Renova: Resets Monday 00:00 UTC");

    // resetsAtUnixSeconds: 0 não deve ser tratado como ciclo expirado
    const zeroReset = computeQuotaPace(
      { id: "weekly", label: "Semanal", usedPercent: 30, remainingPercent: 70, resetsAtUnixSeconds: 0 },
      now,
    );
    expect(zeroReset?.status).toBe("no-reset");
  });

  it("infere duração da janela pelo rótulo ou id quando windowMinutes está ausente", () => {
    expect(resolveWindowMinutes({ id: "session", label: "Sessão" })).toBe(300);
    expect(resolveWindowMinutes({ id: "weekly", label: "Semanal" })).toBe(10080);
    expect(resolveWindowMinutes({ id: "monthly", label: "Mensal" })).toBe(43200);
    expect(resolveWindowMinutes({ id: "custom", label: "Reserva" })).toBeNull();
  });
});

describe("computeRecentHistoryPace e computeCreditsBurnEstimate", () => {
  const now = new Date(2026, 10, 24, 12, 0, 0).getTime();

  it("detecta aceleração recente nos últimos dias frente à média do ciclo", () => {
    const samples = [
      { at: now - 10 * DAY_MS, value: 0 },
      { at: now - 3 * DAY_MS, value: 10 },
      { at: now - 1 * DAY_MS, value: 25 },
      { at: now, value: 40 },
    ];
    const recent = computeRecentHistoryPace(samples, 4, "day", now);
    expect(recent).not.toBeNull();
    expect(recent?.ratePerUnit).toBeCloseTo(10, 1);
    expect(recent?.trend).toBe("faster");
    expect(recent?.windowLabel).toBe("últ. 3d");
  });

  it("detecta burst recente em janela de 5h que esgotaria antes do reset mesmo com média do ciclo ok", () => {
    // 3h decorridas de 5h (faltam 2h): 40% usado -> média 13,3%/h (projeção 67% = ok)
    const resetsAtUnixSeconds = Math.floor((now + 2 * HOUR_MS) / 1000);
    const pace = computeQuotaPace(
      {
        id: "rolling-0",
        label: "Janela atual (5h)",
        usedPercent: 40,
        remainingPercent: 60,
        windowMinutes: 300,
        resetsAtUnixSeconds,
      },
      now,
    );
    expect(pace?.status).toBe("ok");

    const samples = [
      { at: now - 3 * HOUR_MS, value: 0 },
      { at: now - 1 * HOUR_MS, value: 6 },
      { at: now, value: 40 }, // +34% na última 1h -> 34%/h -> 60% restantes acabam em ~1h46 (< 2h!)
    ];
    const recent = computeRecentHistoryPace(samples, pace!.ratePerUnit, "hour", now, pace!.cycleStartMs);
    expect(recent).not.toBeNull();
    expect(recent?.trend).toBe("faster");
    expect(recent?.ratePerUnit).toBeCloseTo(34, 1);
    expect(recent?.windowLabel).toBe("últ. 1h");

    const recentProj = projectPaceAtRecentRate(pace!, recent!, now);
    expect(recentProj?.willDeplete).toBe(true);
    expect(recentProj?.timeToDepleteMs).toBeCloseTo((60 / 34) * HOUR_MS, -3);

    const line = formatMeterPaceLine(pace!, recent, now);
    expect(line?.tone).toBe("ok");
    expect(line?.recentBadge?.trend).toBe("faster");
    expect(line?.recentBadge?.tone).toBe("danger");
    expect(line?.recentBadge?.depletesAtRecentPace).toBe(true);
    expect(line?.recentBadge?.text).toBe("↑ acelerou (~34%/h · esgota antes)");
    expect(line?.recentBadge?.tooltip).toContain("média do ciclo");
    expect(line?.recentBadge?.tooltip).toContain("esgotaria em ~1h 46min");
    expect(line?.tooltip).toContain("esgotaria em ~1h 46min");

    // Burst curto de 45min mostra rótulo honesto em minutos; 15min fica abaixo do piso (null)
    const burst45 = computeRecentHistoryPace(
      [
        { at: now - 45 * 60_000, value: 10 },
        { at: now, value: 35 },
      ],
      pace!.ratePerUnit,
      "hour",
      now,
    );
    expect(burst45?.windowLabel).toBe("últ. 45min");
    const burst15 = computeRecentHistoryPace(
      [
        { at: now - 15 * 60_000, value: 10 },
        { at: now, value: 35 },
      ],
      pace!.ratePerUnit,
      "hour",
      now,
    );
    expect(burst15).toBeNull();
  });

  it("detecta aceleração no mensal e desaceleração (inclusive pausa 0%/dia) no semanal do OpenCode Go", () => {
    // Mensal: 18d decorridos, 12d restantes, 30% usado -> média 1,7%/dia (ok, sobra ~50%)
    // Últimos 3d: +25% -> 8,3%/dia -> 70% restantes esgotam em ~8d 10h (< 12d)
    const monthlyPace = computeQuotaPace(
      {
        id: "monthly-2",
        label: "Mensal",
        usedPercent: 30,
        remainingPercent: 70,
        windowMinutes: 43200,
        resetsAtUnixSeconds: Math.floor((now + 12 * DAY_MS) / 1000),
      },
      now,
    );
    const monthlyRecent = computeRecentHistoryPace(
      [
        { at: now - 15 * DAY_MS, value: 2 },
        { at: now - 3 * DAY_MS, value: 5 },
        { at: now, value: 30 },
      ],
      monthlyPace!.ratePerUnit,
      "day",
      now,
      monthlyPace!.cycleStartMs,
    );
    const monthlyLine = formatMeterPaceLine(monthlyPace!, monthlyRecent, now);
    expect(monthlyLine?.tone).toBe("ok");
    expect(monthlyLine?.recentBadge?.tone).toBe("danger");
    expect(monthlyLine?.recentBadge?.depletesAtRecentPace).toBe(true);
    expect(monthlyLine?.recentBadge?.text).toContain("↑ acelerou (~8,3%/dia · esgota antes)");

    // Semanal: 4d decorridos, 3d restantes, 50% usado -> média 12,5%/dia (tight)
    // Últimos 2d: uso ficou parado em 50% -> 0,0%/dia (desacelerou)
    const weeklyPace = computeQuotaPace(
      {
        id: "weekly-1",
        label: "Semanal",
        usedPercent: 50,
        remainingPercent: 50,
        windowMinutes: 10080,
        resetsAtUnixSeconds: Math.floor((now + 3 * DAY_MS) / 1000),
      },
      now,
    );
    const weeklyRecent = computeRecentHistoryPace(
      [
        { at: now - 4 * DAY_MS, value: 5 },
        { at: now - 2 * DAY_MS, value: 50 },
        { at: now - 1 * DAY_MS, value: 50 },
        { at: now, value: 50 },
      ],
      weeklyPace!.ratePerUnit,
      "day",
      now,
      weeklyPace!.cycleStartMs,
    );
    expect(weeklyRecent?.ratePerUnit).toBe(0);
    expect(weeklyRecent?.trend).toBe("slower");
    const weeklyLine = formatMeterPaceLine(weeklyPace!, weeklyRecent, now);
    expect(weeklyLine?.recentBadge?.trend).toBe("slower");
    expect(weeklyLine?.recentBadge?.tone).toBe("ok");
    expect(weeklyLine?.recentBadge?.text).toBe("↓ desacelerou");
    expect(weeklyLine?.recentBadge?.tooltip).toContain("0,0%/dia");
  });

  it("ignora histórico obsoleto, descarta ciclo anterior após reset e omite badge quando ritmo é estável", () => {
    // Amostras com mais de 3d de atraso não representam ritmo recente
    const stale = computeRecentHistoryPace(
      [
        { at: now - 8 * DAY_MS, value: 10 },
        { at: now - 5 * DAY_MS, value: 40 },
      ],
      5,
      "day",
      now,
    );
    expect(stale).toBeNull();

    // Queda de reset (90% -> 5%) dentro da janela recente mede apenas o segmento pós-reset
    const afterReset = computeRecentHistoryPace(
      [
        { at: now - 3 * DAY_MS, value: 90 },
        { at: now - 2 * DAY_MS, value: 5 },
        { at: now, value: 35 }, // +30% em 2d = 15%/dia
      ],
      6,
      "day",
      now,
    );
    expect(afterReset?.ratePerUnit).toBeCloseTo(15, 1);
    expect(afterReset?.trend).toBe("faster");

    // Ritmo recente estável não gera badge na barra
    const pace = computeQuotaPace(
      {
        id: "weekly",
        label: "Semanal",
        usedPercent: 30,
        remainingPercent: 70,
        windowMinutes: 10080,
        resetsAtUnixSeconds: Math.floor((now + 4 * DAY_MS) / 1000), // 3d decorridos -> 10%/dia
      },
      now,
    );
    const steady = computeRecentHistoryPace(
      [
        { at: now - 2 * DAY_MS, value: 10 },
        { at: now, value: 30 }, // 10%/dia -> steady
      ],
      pace!.ratePerUnit,
      "day",
      now,
      pace!.cycleStartMs,
    );
    expect(steady?.trend).toBe("steady");
    expect(formatMeterPaceLine(pace!, steady, now)?.recentBadge).toBeNull();
  });

  it("calcula ritmo de gasto em saldo USD ignorando recargas", () => {
    const samples = [
      { at: now - 4 * DAY_MS, value: 10.0 },
      { at: now - 3 * DAY_MS, value: 8.0 }, // gastou 2
      { at: now - 2 * DAY_MS, value: 18.0 }, // recarga de +10 (ignorada)
      { at: now, value: 14.0 }, // gastou 4 → total 6 em 4 dias = 1,5/dia
    ];
    const burn = computeCreditsBurnEstimate(samples);
    expect(burn).not.toBeNull();
    expect(burn?.ratePerDay).toBeCloseTo(1.5, 2);
    expect(burn?.daysRemaining).toBeCloseTo(14 / 1.5, 1);
  });
});

describe("formatadores pt-BR de ritmo e reset", () => {
  it("formata taxas, durações e datas de reset de forma consistente", () => {
    expect(formatPaceRate(0.02, "hour")).toBe("<0,1%/h");
    expect(formatPaceRate(3.04, "day")).toBe("3,0%/dia");
    expect(formatPaceRate(17.9, "hour")).toBe("18%/h");
    expect(formatPaceDuration(45 * 60_000)).toBe("45min");
    expect(formatPaceDuration(2 * HOUR_MS + 15 * 60_000)).toBe("2h 15min");
    expect(formatPaceDuration(36 * HOUR_MS)).toBe("1d 12h");
    expect(formatCompactDuration(36 * HOUR_MS)).toBe("1d12h");

    const base = new Date(2026, 10, 24, 10, 0, 0).getTime();
    expect(formatResetDateTime(new Date(2026, 10, 24, 14, 30, 0).getTime(), base)).toBe("hoje, 14:30");
    expect(formatResetDateTime(new Date(2026, 10, 25, 8, 0, 0).getTime(), base)).toBe("amanhã, 08:00");
    expect(formatResetDateTime(new Date(2026, 10, 28, 9, 15, 0).getTime(), base)).toBe("28/11, 09:15");
  });
});
