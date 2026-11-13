import { describe, expect, it } from "vitest";
import { normalizeForGrouping, parseJsonl, parseSince, summarize } from "./debug-log-summary.mjs";

const base = {
  category: "telemetry",
  source: "rust-backend",
  appVersion: "1.6.0",
  sessionStartedUnixMs: 1,
};

describe("debug-log-summary", () => {
  it("interpreta janelas relativas e datas ISO", () => {
    const now = Date.UTC(2026, 10, 13, 12);
    expect(parseSince("2h", now)).toBe(now - 2 * 3600_000);
    expect(parseSince("7d", now)).toBe(now - 7 * 86_400_000);
    expect(parseSince("2026-11-13T00:00:00Z", now)).toBe(Date.UTC(2026, 10, 13));
    expect(() => parseSince("ontem", now)).toThrow(/--since inválido/);
  });

  it("conta linhas corrompidas em vez de descartá-las em silêncio", () => {
    const { entries, invalid } = parseJsonl('{"a":1}\n{quebrado\n\n{"b":2}\r\n');
    expect(entries).toHaveLength(2);
    expect(invalid).toBe(1);
  });

  it("agrupa falhas iguais mesmo com números variáveis", () => {
    expect(normalizeForGrouping("line 1 column 5284")).toBe(normalizeForGrouping("line 1 column 5301"));
  });

  it("resume operações e agrupa erros por operação, etapa e detalhe", () => {
    const entries = [
      { ...base, timestampUnixMs: 10, operation: "system_overview", outcome: "error", durationMs: 5800, message: "Falha · 1ª seguida", stage: "parse", detail: "control character at line 1 column 5284", context: "Battle\\u{0007}net" },
      { ...base, timestampUnixMs: 20, operation: "system_overview", outcome: "error", durationMs: 5900, message: "Falha · 2ª seguida", stage: "parse", detail: "control character at line 1 column 5290" },
      { ...base, timestampUnixMs: 30, operation: "performance_details", outcome: "ok", durationMs: 1500, message: "Concluído" },
      { ...base, timestampUnixMs: 1, operation: "system_overview", outcome: "ok", durationMs: 100, message: "antes da janela" },
    ];
    const summary = summarize(entries, { sinceMs: 5 });
    expect(summary.entryCount).toBe(3);
    expect(summary.errorCount).toBe(2);
    expect(summary.operations[0]).toMatchObject({ operation: "system_overview", total: 2, errors: 2, errorRate: 1 });
    expect(summary.errorGroups).toHaveLength(1);
    expect(summary.errorGroups[0]).toMatchObject({ count: 2, stage: "parse", message: "Falha · 2ª seguida", context: "Battle\\u{0007}net", versions: ["1.6.0"] });
  });

  it("filtra por operação e só erros", () => {
    const entries = [
      { ...base, timestampUnixMs: 10, operation: "recent_files", outcome: "ok", durationMs: 5, message: "ok" },
      { ...base, timestampUnixMs: 11, operation: "recent_files", outcome: "error", durationMs: 5, message: "Falha na consulta" },
      { ...base, timestampUnixMs: 12, operation: "system_overview", outcome: "error", durationMs: 5, message: "Falha" },
    ];
    const summary = summarize(entries, { operation: "recent", errorsOnly: true });
    expect(summary.entryCount).toBe(1);
    expect(summary.recentErrors[0].operation).toBe("recent_files");
  });
});
