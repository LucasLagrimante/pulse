import { Search, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { ProcessSortColumn, ProcessSortDirection, TopProcess } from "./App";
import { ProcessFallbackIcon } from "./Identity";
import "./DataTable.css";
import "./ActivityProcessTable.css";

const SORT_COLUMNS: { id: ProcessSortColumn; label: string; name: string; hint: string }[] = [
  { id: "memory", label: "RAM", name: "memória", hint: "Memória residente somada do grupo." },
  {
    id: "cpu",
    label: "CPU",
    name: "CPU",
    hint: "CPU agregada do grupo, normalizada pela máquina (100% = todos os processadores lógicos).",
  },
  { id: "gpu", label: "GPU", name: "GPU", hint: "GPU agregada do grupo, somando as engines reportadas pelo Windows." },
];

const GPU_UNAVAILABLE = "Indisponível: o driver não expôs contadores de engine gráfico (PDH).";

// Formata métricas sem inventar zero: ausência vira "—" (com justificativa no title da célula)
function formatMetric(value: number | null | undefined, suffix: string): string {
  if (value == null || typeof value !== "number" || Number.isNaN(value)) return "—";
  return `${value.toLocaleString("pt-BR", { maximumFractionDigits: 1 })}${suffix}`;
}

// Memória acima de 1 GB fica mais legível em GB
function formatMemory(mb: number | null | undefined): string {
  if (mb == null || typeof mb !== "number" || Number.isNaN(mb)) return "—";
  return mb >= 1024 ? formatMetric(mb / 1024, " GB") : formatMetric(Math.round(mb), " MB");
}

// Percentual 0–100 para a barra; null quando não há base honesta para desenhar
function barPercent(value: number | null | undefined, total = 100): number | null {
  if (value == null || typeof value !== "number" || Number.isNaN(value) || total <= 0) return null;
  return Math.max(0, Math.min(100, (value / total) * 100));
}

function isProtectedProcess(process: TopProcess) {
  return process.name.toLocaleLowerCase("pt-BR").startsWith("pulse");
}

function UsageCell({
  value,
  percent,
  tone,
  title,
}: {
  value: string;
  percent: number | null;
  tone: "memory" | "cpu" | "gpu";
  title: string;
}) {
  const hot = tone !== "memory" && percent != null && percent >= 50;
  return (
    <span className={`dt-usage dt-usage--${tone}${hot ? " is-hot" : ""}`} title={title}>
      <span className="dt-usage-value">{value}</span>
      {percent != null && (
        <span className="dt-usage-track" aria-hidden="true">
          <span className="dt-usage-fill" style={{ width: `${Math.max(percent, 1.5)}%` }} />
        </span>
      )}
    </span>
  );
}

export function ActivityProcessTable({
  processes,
  limit,
  memoryTotalGb,
  onTerminate,
  terminatingId,
  sortBy,
  sortDirection,
  onSortChange,
}: {
  processes: TopProcess[];
  limit: number;
  memoryTotalGb?: number | null;
  onTerminate: (process: TopProcess) => void;
  terminatingId?: number | null;
  sortBy: ProcessSortColumn;
  sortDirection: ProcessSortDirection;
  onSortChange: (column: ProcessSortColumn) => void;
}) {
  const [query, setQuery] = useState("");
  const gpuAvailable = processes.some(process => process.gpuPercent != null);
  const memoryTotalMb = typeof memoryTotalGb === "number" && memoryTotalGb > 0 ? memoryTotalGb * 1024 : null;

  const visible = useMemo(() => {
    const term = query.trim().toLocaleLowerCase("pt-BR");
    const filtered = term
      ? processes.filter(process =>
          [process.displayName, process.name, process.publisher]
            .filter(Boolean)
            .some(text => String(text).toLocaleLowerCase("pt-BR").includes(term)),
        )
      : processes;
    return filtered.slice(0, limit);
  }, [processes, query, limit]);

  const totalMemoryMb = visible.reduce((sum, process) => sum + (Number.isFinite(process.memoryMb) ? process.memoryMb : 0), 0);
  const measuredCpu = visible.filter(process => typeof process.cpuPercent === "number" && !Number.isNaN(process.cpuPercent));
  const totalCpu = measuredCpu.reduce((sum, process) => sum + (process.cpuPercent as number), 0);

  return (
    <div className="dt dt--activity">
      <div className="dt-toolbar">
        <label className="dt-search">
          <Search size={15} aria-hidden="true" />
          <input
            type="search"
            value={query}
            placeholder="Filtrar por nome, executável ou editor"
            aria-label="Filtrar aplicativos em execução"
            onChange={event => setQuery(event.target.value)}
          />
        </label>
        <dl className="dt-summary" aria-label="Resumo dos grupos exibidos">
          <div>
            <dt>Grupos</dt>
            <dd>{visible.length}</dd>
          </div>
          <div>
            <dt>RAM somada</dt>
            <dd>{visible.length ? formatMemory(totalMemoryMb) : "—"}</dd>
          </div>
          <div>
            <dt>CPU somada</dt>
            <dd>{measuredCpu.length ? formatMetric(totalCpu, " %") : "—"}</dd>
          </div>
        </dl>
      </div>

      <div className="dt-table">
        <div className="dt-head">
          <span className="dt-center" aria-hidden="true">#</span>
          <span aria-hidden="true">Aplicativo</span>
          {SORT_COLUMNS.map(({ id, label, name, hint }) => {
            const isActive = sortBy === id;
            const unavailable = id === "gpu" && !gpuAvailable;
            return (
              <button
                key={id}
                type="button"
                className={`dt-sort${isActive ? " is-active" : ""}`}
                aria-label={`Ordenar por ${name}${isActive ? (sortDirection === "desc" ? ", decrescente" : ", crescente") : ""}`}
                aria-pressed={isActive}
                disabled={unavailable}
                title={unavailable ? GPU_UNAVAILABLE : hint}
                onClick={() => onSortChange(id)}
              >
                {unavailable ? "GPU —" : label}
                {isActive && (
                  <span className="dt-sort-arrow" aria-hidden="true">
                    {sortDirection === "desc" ? "↓" : "↑"}
                  </span>
                )}
              </button>
            );
          })}
          <span className="dt-center" aria-hidden="true">Ação</span>
        </div>

        {visible.length === 0 ? (
          <div className="dt-empty" role="status">
            {query.trim() ? (
              <>
                <p>Nenhum aplicativo corresponde a “{query.trim()}”.</p>
                <button type="button" className="dt-empty-reset" onClick={() => setQuery("")}>
                  Limpar filtro
                </button>
              </>
            ) : (
              <p>Coletando aplicativos em execução…</p>
            )}
          </div>
        ) : (
          <ol className="dt-list">
            {visible.map((process, index) => {
              const label = process.displayName || process.name;
              const count = process.processCount ?? 1;
              const isClosing = terminatingId === process.id;
              const isProtected = isProtectedProcess(process);
              const details = [process.name, process.publisher].filter(Boolean).join(" · ") || `PID ${process.id}`;
              return (
                <li
                  key={`${process.id}-${process.name}`}
                  className={`dt-row row-hl${isClosing ? " is-busy" : ""}${isProtected ? " is-protected" : ""}`}
                >
                  <span className="dt-rank">{String(index + 1).padStart(2, "0")}</span>
                  <div className="dt-identity" title={[label, process.name, process.publisher, process.path].filter(Boolean).join(" · ")}>
                    <span className={`dt-icon${process.icon ? "" : " is-empty"}`} aria-hidden="true">
                      {process.icon ? <img src={process.icon} alt="" /> : <ProcessFallbackIcon kind={process.systemKind} />}
                    </span>
                    <span className="dt-name">
                      <strong>{label}</strong>
                      <small>
                        {details}
                        {count > 1 && <span className="dt-badge">{count} processos</span>}
                      </small>
                    </span>
                  </div>
                  <UsageCell
                    tone="memory"
                    value={formatMemory(process.memoryMb)}
                    percent={memoryTotalMb ? barPercent(process.memoryMb, memoryTotalMb) : null}
                    title={memoryTotalMb ? "Memória residente do grupo; a barra mostra a fatia da RAM total" : "Memória residente do grupo"}
                  />
                  <UsageCell
                    tone="cpu"
                    value={formatMetric(process.cpuPercent, "%")}
                    percent={barPercent(process.cpuPercent)}
                    title="CPU agregada do grupo desde a coleta anterior"
                  />
                  <UsageCell
                    tone="gpu"
                    value={formatMetric(process.gpuPercent, "%")}
                    percent={barPercent(process.gpuPercent)}
                    title={
                      process.gpuPercent == null
                        ? `GPU ${GPU_UNAVAILABLE.toLocaleLowerCase("pt-BR")}`
                        : "GPU agregada do grupo; 0% indica medição válida sem uso."
                    }
                  />
                  <span className="dt-action">
                    <button
                      type="button"
                      className="row-action row-hl-danger"
                      aria-label={`Fechar ${label}`}
                      title={
                        isProtected
                          ? "O Pulse não encerra a si mesmo"
                          : `Fechar ${label}${count > 1 ? ` (${count} processos)` : ""}`
                      }
                      disabled={isClosing || isProtected}
                      onClick={() => onTerminate(process)}
                    >
                      <X size={14} strokeWidth={2.2} aria-hidden="true" />
                      <span>{isClosing ? "Fechando…" : "Fechar"}</span>
                    </button>
                  </span>
                </li>
              );
            })}
          </ol>
        )}
      </div>
    </div>
  );
}
