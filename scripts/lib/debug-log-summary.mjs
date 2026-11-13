// Funções puras de leitura e resumo dos logs de auditoria do Pulse
// (`%LOCALAPPDATA%\Pulse\logs\debug-AAAA-MM-DD.jsonl`). Usadas por
// scripts/debug-logs.mjs e cobertas por debug-log-summary.test.mjs.

/** Converte `24h`, `7d`, `90m` ou uma data ISO em timestamp (ms). */
export function parseSince(value, nowMs = Date.now()) {
  if (!value) return nowMs - 24 * 3600_000;
  const match = /^(\d+)\s*([mhd])$/i.exec(String(value).trim());
  if (match) {
    const amount = Number(match[1]);
    const unit = { m: 60_000, h: 3600_000, d: 86_400_000 }[match[2].toLowerCase()];
    return nowMs - amount * unit;
  }
  const parsed = Date.parse(value);
  if (Number.isNaN(parsed)) throw new Error(`--since inválido: ${value} (use 90m, 24h, 7d ou data ISO)`);
  return parsed;
}

/** Lê linhas JSONL tolerando linhas corrompidas (contadas, não descartadas em silêncio). */
export function parseJsonl(text) {
  const entries = [];
  let invalid = 0;
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    try {
      entries.push(JSON.parse(line));
    } catch {
      invalid += 1;
    }
  }
  return { entries, invalid };
}

/** Números variáveis (PIDs, colunas, bytes, contagens) não podem separar falhas iguais. */
export function normalizeForGrouping(text) {
  return String(text ?? "").replace(/\d+/g, "#").slice(0, 160);
}

function percentile(sorted, p) {
  if (!sorted.length) return null;
  return sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))];
}

export function summarize(entries, { sinceMs = 0, operation = null, errorsOnly = false } = {}) {
  const filtered = entries
    .filter(entry => Number(entry.timestampUnixMs) >= sinceMs)
    .filter(entry => !operation || String(entry.operation).includes(operation))
    .filter(entry => !errorsOnly || entry.outcome === "error")
    .sort((a, b) => a.timestampUnixMs - b.timestampUnixMs);

  const operations = new Map();
  const errorGroups = new Map();
  const versions = new Map();
  const sessions = new Set();

  for (const entry of filtered) {
    if (entry.appVersion) versions.set(entry.appVersion, (versions.get(entry.appVersion) ?? 0) + 1);
    if (entry.sessionStartedUnixMs) sessions.add(entry.sessionStartedUnixMs);

    const op = operations.get(entry.operation) ?? { operation: entry.operation, total: 0, errors: 0, durations: [], lastOutcome: null, lastAt: 0 };
    op.total += 1;
    if (entry.outcome === "error") op.errors += 1;
    if (Number.isFinite(entry.durationMs)) op.durations.push(entry.durationMs);
    op.lastOutcome = entry.outcome;
    op.lastAt = entry.timestampUnixMs;
    operations.set(entry.operation, op);

    if (entry.outcome === "error") {
      const key = [entry.operation, entry.stage ?? "-", normalizeForGrouping(entry.detail ?? entry.message)].join("|");
      const group = errorGroups.get(key) ?? {
        operation: entry.operation,
        stage: entry.stage ?? null,
        message: entry.message,
        detail: entry.detail ?? null,
        context: entry.context ?? null,
        count: 0,
        firstAt: entry.timestampUnixMs,
        lastAt: entry.timestampUnixMs,
        versions: new Set(),
      };
      group.count += 1;
      group.lastAt = entry.timestampUnixMs;
      // Mantém o exemplo mais recente: é o que reflete o estado atual.
      group.message = entry.message;
      group.detail = entry.detail ?? group.detail;
      group.context = entry.context ?? group.context;
      if (entry.appVersion) group.versions.add(entry.appVersion);
      errorGroups.set(key, group);
    }
  }

  const operationRows = [...operations.values()]
    .map(op => {
      const sorted = [...op.durations].sort((a, b) => a - b);
      return {
        operation: op.operation,
        total: op.total,
        errors: op.errors,
        errorRate: op.total ? op.errors / op.total : 0,
        p50Ms: percentile(sorted, 50),
        p95Ms: percentile(sorted, 95),
        lastOutcome: op.lastOutcome,
        lastAt: op.lastAt,
      };
    })
    .sort((a, b) => b.errors - a.errors || b.total - a.total);

  const groups = [...errorGroups.values()]
    .map(group => ({ ...group, versions: [...group.versions] }))
    .sort((a, b) => b.count - a.count);

  return {
    entryCount: filtered.length,
    errorCount: filtered.filter(entry => entry.outcome === "error").length,
    versions: Object.fromEntries(versions),
    sessionCount: sessions.size,
    operations: operationRows,
    errorGroups: groups,
    recentErrors: filtered.filter(entry => entry.outcome === "error").slice(-10),
  };
}
