import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import "./ServicesHub.css";

/* ------------------------------------------------------------------ */
/* Tipos espelhados do Rust (serde camelCase)                          */
/* ------------------------------------------------------------------ */

export type WindowsService = {
  name: string;
  displayName: string;
  state: string;
  stateCode: number;
  startType: string;
  startTypeCode: number;
  processId: number | null;
  executablePath: string | null;
  isUserService: boolean;
  icon: string | null;
  /** Conta de execução do serviço (`LocalSystem`, `DOMINIO\usuario`, ...). */
  startName: string | null;
  /** Grupo calculado no Rust. `"unknown"` = sem dado suficiente para afirmar. */
  category: ServiceCategory;
  /**
   * Fabricante declarado nos recursos de versão do executável. **Não é
   * assinatura validada** — é auto-declarado pelo fabricante, então nunca
   * aparece sozinho como motivo de suspeita.
   */
  publisher: string | null;
  /**
   * Evidências que levaram o serviço a `"review"`. Vazio = nenhum sinal.
   * Sempre textual e verificável; o backend jamais rotula malware.
   */
  suspicionReasons: string[];
};

export type ServicesSnapshot = {
  services: WindowsService[];
  runningCount: number;
  stoppedCount: number;
  pausedCount: number;
  totalCount: number;
  unavailableReason: string | null;
  collectedAtUnixMs: number;
};

export type PendingUpdate = {
  id: string;
  name: string;
  currentVersion: string;
  availableVersion: string;
  source: string;
};

export type UpdatesSnapshot = {
  updates: PendingUpdate[];
  totalCount: number;
  unavailableReason: string | null;
  collectedAtUnixMs: number;
};

export type ServiceFilter = "all" | "running" | "stopped" | "paused";
/** Filtro de categoria: `"all"` não restringe. */
export type ServiceCategoryFilter = "all" | ServiceCategory;
/** Espelha o `category` do Rust. `"unknown"` = sem `StartName` nem executável. */
export type ServiceCategory = "system" | "user" | "review" | "unknown";
export type HubTab = "services" | "updates";

/* ------------------------------------------------------------------ */
/* Helpers puros (testáveis sem DOM)                                   */
/* ------------------------------------------------------------------ */

const UNKNOWN_NUMBER = "Indisponível";

/**
 * Formata um contador. `null`/`NaN` nunca viram `0`: ausência de dado é
 * "Indisponível", não zero — zero é um valor real e significa outra coisa.
 */
export function formatCount(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return UNKNOWN_NUMBER;
  return new Intl.NumberFormat("pt-BR").format(value);
}

/** PID só aparece quando o serviço realmente está executando. */
export function formatPid(service: WindowsService): string {
  const pid = service.processId;
  if (service.state !== "Em execução") return "Processo inativo";
  if (pid == null || !Number.isFinite(pid)) return "PID indisponível";
  return `PID ${pid}`;
}

export function serviceLabel(service: WindowsService): string {
  return service.displayName.trim() || service.name;
}

export function matchesServiceFilter(service: WindowsService, filter: ServiceFilter): boolean {
  switch (filter) {
    case "running":
      return service.state === "Em execução";
    case "stopped":
      return service.state === "Parado";
    case "paused":
      return service.state === "Pausado";
    default:
      return true;
  }
}

/**
 * Rótulo do grupo. "Revisão" e nunca "suspeito"/"malware": o backend só
 * entrega evidências verificáveis, e a palavra de acusação não aparece em
 * lugar nenhum da interface.
 */
export function categoryLabel(category: ServiceCategory): string {
  switch (category) {
    case "system":
      return "Sistema";
    case "user":
      return "Usuário";
    case "review":
      return "Revisão";
    default:
      return "Sem dados";
  }
}

/**
 * O filtro "Usuário" usa `isUserService`, não `category`: um serviço de usuário
 * em diretório gravável cai em `category === "review"` (revisão vence), e ele
 * continua sendo do usuário — perdê-lo de vista ali seria esconder a origem.
 */
export function matchesServiceCategory(
  service: WindowsService,
  category: ServiceCategoryFilter,
): boolean {
  if (category === "all") return true;
  if (category === "user") return service.isUserService;
  return service.category === category;
}

/**
 * Texto do badge: a categoria **e** a evidência que a justifica. Sem conta nem
 * executável, o tooltip diz isso — em vez de sugerir que o serviço é do
 * sistema por omissão.
 */
export function categoryTitle(service: WindowsService): string {
  const label = categoryLabel(service.category);
  if (service.category === "unknown") {
    return `${label}: o Windows não informou a conta de execução nem o executável`;
  }
  const parts = [`${label}`];
  if (service.startName) parts.push(`conta: ${service.startName}`);
  if (service.publisher) parts.push(`fabricante declarado: ${service.publisher}`);
  const reasons = service.suspicionReasons ?? [];
  if (reasons.length > 0) parts.push(`sinais: ${reasons.join("; ")}`);
  return parts.join(" — ");
}

/** Contagem por categoria, para os chips. Sempre do total real, nunca zero falso. */
export function countByCategory(services: WindowsService[]): Record<ServiceCategory, number> {
  const counts: Record<ServiceCategory, number> = { system: 0, user: 0, review: 0, unknown: 0 };
  for (const service of services) {
    // `category` pode vir fora do conjunto se o backend mudar: contar como
    // "sem dados" é honesto, quebrar a lista não é. `Object.hasOwn` exigiria
    // lib es2022 e o tsconfig do projeto ainda não a usa.
    const key = service.category in counts ? service.category : "unknown";
    counts[key] += 1;
  }
  return counts;
}

/** Busca case-insensitive em nome, nome de exibição, caminho, conta e motivo. */
export function matchesServiceQuery(service: WindowsService, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return (
    service.name.toLowerCase().includes(needle) ||
    service.displayName.toLowerCase().includes(needle) ||
    (service.executablePath ?? "").toLowerCase().includes(needle) ||
    (service.startName ?? "").toLowerCase().includes(needle) ||
    service.suspicionReasons.some(reason => reason.toLowerCase().includes(needle))
  );
}

/**
 * `category` é opcional e padrão `"all"` de propósito: os filtros por estado já
 * são o contrato dos testes e das chamadas existentes, e categoria é um eixo
 * adicional — não uma troca de assinatura.
 */
export function filterServices(
  services: WindowsService[],
  query: string,
  filter: ServiceFilter,
  category: ServiceCategoryFilter = "all",
): WindowsService[] {
  return sortServices(
    services.filter(
      s =>
        matchesServiceFilter(s, filter) &&
        matchesServiceCategory(s, category) &&
        matchesServiceQuery(s, query),
    ),
  );
}

const STATE_RANK: Record<string, number> = {
  "Em execução": 0,
  Iniciando: 1,
  Retomando: 1,
  "Pausado": 2,
  Pausando: 2,
  Parando: 3,
  Parado: 4,
};

/** Estados ativos primeiro; dentro de cada grupo, ordem alfabética estável. */
export function sortServices(services: WindowsService[]): WindowsService[] {
  return [...services].sort((a, b) => {
    const rankDiff = (STATE_RANK[a.state] ?? 9) - (STATE_RANK[b.state] ?? 9);
    if (rankDiff !== 0) return rankDiff;
    return serviceLabel(a).localeCompare(serviceLabel(b), "pt-BR");
  });
}

const FALLBACK_ICON =
  "data:image/svg+xml;utf8," +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" rx="5" fill="#23282b"/><path d="M12 4v16M4 12h16" stroke="#8b928a" stroke-width="2.4" stroke-linecap="round"/></svg>',
  );

/* ------------------------------------------------------------------ */
/* Componente                                                          */
/* ------------------------------------------------------------------ */

type LoadState = "idle" | "loading" | "ready";

/**
 * Estado de uma atualização disparada pelo usuário. Cada item tem o seu
 * próprio: cliques em vários pacotes correm em paralelo e cada linha mostra o
 * que está acontecendo com ela — nada de loading global que o clique seguinte
 * cancela. `message` carrega a mensagem original do winget/Windows no erro.
 */
export type UpdateJob = {
  state: "running" | "done" | "error";
  /** Última linha emitida pelo winget para este pacote. */
  log?: string;
  message?: string;
  startedAt?: number;
  settledAt?: number;
};

/** Evento de progresso emitido pelo backend a cada linha de saída do winget. */
type UpgradeProgress = { id: string; phase: string; line: string };

/** Id reservado do backend para a atualização em lote (`winget upgrade --all`). */
const ALL_PACKAGES_ID = "*";

/** `mm:ss` para o tempo decorrido da instalação. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

export function ServicesHub() {
  // Atualizações abre primeiro: é a informação acionável (o que está
  // desatualizado) e não depende de privilégio administrativo para aparecer.
  const [tab, setTab] = useState<HubTab>("updates");

  const [services, setServices] = useState<ServicesSnapshot | null>(null);
  const [servicesState, setServicesState] = useState<LoadState>("idle");
  const [servicesError, setServicesError] = useState("");

  const [updates, setUpdates] = useState<UpdatesSnapshot | null>(null);
  const [updatesState, setUpdatesState] = useState<LoadState>("idle");
  const [updatesError, setUpdatesError] = useState("");

  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ServiceFilter>("all");
  const [category, setCategory] = useState<ServiceCategoryFilter>("all");
  const [busyService, setBusyService] = useState("");
  const [updateJobs, setUpdateJobs] = useState<Record<string, UpdateJob>>({});
  const [busyAll, setBusyAll] = useState(false);
  const [batchLog, setBatchLog] = useState("");
  const [batchStartedAt, setBatchStartedAt] = useState(0);
  const [notice, setNotice] = useState("");

  // Espelho síncrono dos pacotes em execução: o `state` do React é assíncrono
  // e dois cliques em sequência precisam ver o id já reservado, senão a mesma
  // atualização é disparada duas vezes.
  const runningUpdates = useRef(new Set<string>());

  const runningCount = useMemo(
    () => Object.values(updateJobs).filter(job => job.state === "running").length,
    [updateJobs],
  );

  // Relógio de 1 s, ativo só enquanto algo instala: é o que faz o tempo
  // decorrido andar sem sujar a lista inteira de re-renders.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (runningCount === 0 && !busyAll) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [runningCount, busyAll]);

  /**
   * Progresso do backend. Uma linha por evento: o winget não expõe
   * porcentagem, então a barra é indeterminada e o texto é o feedback real.
   */
  useEffect(() => {
    let active = true;
    const unlistenPromise = listen<UpgradeProgress>("winget-upgrade-progress", event => {
      const { id, phase, line } = event.payload;
      if (id === ALL_PACKAGES_ID) {
        setBatchLog(line);
        return;
      }
      setUpdateJobs(prev => {
        const job = prev[id];
        // Evento de uma operação que a lista já não exibe: descartar.
        if (!job) return prev;
        if (phase === "finished") return { ...prev, [id]: { ...job, log: "Concluído" } };
        return { ...prev, [id]: { ...job, log: line } };
      });
    });
    return () => {
      active = false;
      void unlistenPromise.then(unlisten => {
        if (active) unlisten();
      });
    };
  }, []);

  // Um job que não está mais na lista de pendências já cumpriu seu papel.
  useEffect(() => {
    if (updates == null) return;
    const live = new Set(updates.updates.map(update => update.id));
    setUpdateJobs(prev => {
      const next: Record<string, UpdateJob> = {};
      for (const [id, job] of Object.entries(prev)) {
        if (job.state === "running" || live.has(id)) next[id] = job;
      }
      return Object.keys(next).length === Object.keys(prev).length ? prev : next;
    });
  }, [updates]);

  const loadServices = useCallback(async (forceRefresh: boolean) => {
    setServicesState("loading");
    setServicesError("");
    try {
      const next = await invoke<ServicesSnapshot>("list_windows_services", { forceRefresh });
      setServices(next);
      setServicesState("ready");
    } catch (cause) {
      setServicesError(String(cause));
      setServicesState("ready");
    }
  }, []);

  const loadUpdates = useCallback(async (forceRefresh: boolean) => {
    setUpdatesState("loading");
    setUpdatesError("");
    try {
      const next = await invoke<UpdatesSnapshot>("list_pending_updates", { forceRefresh });
      setUpdates(next);
      setUpdatesState("ready");
    } catch (cause) {
      setUpdatesError(String(cause));
      setUpdatesState("ready");
    }
  }, []);

  useEffect(() => {
    if (servicesState === "idle") void loadServices(false);
  }, [servicesState, loadServices]);

  useEffect(() => {
    if (tab === "updates" && updatesState === "idle") void loadUpdates(false);
  }, [tab, updatesState, loadUpdates]);

  const categoryCounts = useMemo(
    () => countByCategory(services?.services ?? []),
    [services],
  );

  const visibleServices = useMemo(
    () => filterServices(services?.services ?? [], query, filter, category),
    [services, query, filter, category],
  );

  const runServiceAction = async (service: WindowsService, action: "start" | "stop") => {
    setBusyService(service.name);
    setServicesError("");
    setNotice("");
    try {
      const next = await invoke<ServicesSnapshot>("set_windows_service_state", {
        name: service.name,
        action,
      });
      setServices(next);
      setNotice(
        action === "start"
          ? `O serviço ${serviceLabel(service)} foi iniciado.`
          : `O serviço ${serviceLabel(service)} foi parado.`,
      );
    } catch (cause) {
      // O Windows recusa com "Acesso negado..." quando o Pulse não é admin.
      // A mensagem original é exibida; nada é engolido.
      setServicesError(String(cause));
    } finally {
      setBusyService("");
    }
  };

  /**
   * Dispara a atualização de **um** pacote. Cada clique é independente: os
   * outros itens continuam com o próprio loading, barra e log, e a lista só é
   * reconsultada quando não houver mais nada em execução (reconsultar a cada
   * item derruba a lista e mistura estados visuais).
   */
  const runUpgrade = async (update: PendingUpdate) => {
    if (runningUpdates.current.has(update.id)) return;
    runningUpdates.current.add(update.id);
    setUpdateJobs(prev => ({ ...prev, [update.id]: { state: "running", log: "", startedAt: Date.now() } }));
    let failure: string | null = null;
    try {
      await invoke<string>("upgrade_winget_package", { id: update.id });
    } catch (cause) {
      failure = String(cause);
    } finally {
      runningUpdates.current.delete(update.id);
    }
    const settledAt = Date.now();
    setUpdateJobs(prev => {
      const started = prev[update.id]?.startedAt ?? settledAt;
      if (failure == null) {
        return { ...prev, [update.id]: { state: "done", log: "Concluído", startedAt: started, settledAt } };
      }
      return {
        ...prev,
        [update.id]: { state: "error", message: failure, log: failure, startedAt: started, settledAt },
      };
    });
    // Só recarrega quando a última instalação em curso terminou.
    if (failure == null && runningUpdates.current.size === 0) await loadUpdates(true);
  };

  /** Atualiza todos os pacotes desatualizados em uma única execução do winget. */
  const runUpgradeAll = async () => {
    if (busyAll) return;
    setBusyAll(true);
    setBatchStartedAt(Date.now());
    setBatchLog("");
    setUpdatesError("");
    setNotice("");
    try {
      await invoke<string>("upgrade_all_winget_packages");
      setNotice("Todos os aplicativos desatualizados foram enviados para atualização.");
      await loadUpdates(true);
    } catch (cause) {
      setUpdatesError(String(cause));
    } finally {
      setBusyAll(false);
    }
  };

  /**
   * "Atualizar tudo" na aba de Serviços reconsulta os dois inventários:
   * serviços e atualizações pendentes. Nada é iniciado/parado em lote —
   * mudança de estado em massa é destrutiva e depende de elevação.
   */
  const refreshEverything = async () => {
    setNotice("");
    await Promise.all([loadServices(true), loadUpdates(true)]);
  };

  const batchElapsed = batchStartedAt > 0 ? formatElapsed(now - batchStartedAt) : "00:00";

  const servicesUnavailable = services?.unavailableReason != null;
  const updatesUnavailable = updates?.unavailableReason != null;

  /** Ordem dos chips de categoria: revelação do sinal primeiro. */
  const CATEGORY_CHIPS: { value: ServiceCategoryFilter; label: string }[] = [
    { value: "all", label: "Todos" },
    { value: "system", label: "Sistema" },
    { value: "user", label: "Usuário" },
    { value: "review", label: "Revisão" },
    { value: "unknown", label: "Sem dados" },
  ];

  return (
    <section className="module-dashboard sh-hub" aria-label="Serviços e atualizações">
      <nav className="storage-tabs-bar" role="tablist" aria-label="Serviços e atualizações">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "updates"}
          className={`storage-tab-btn ${tab === "updates" ? "active" : ""}`}
          onClick={() => setTab("updates")}
        >
          <span>ATUALIZAÇÕES</span>
          {updates ? (
            <span className="storage-tab-badge">{updates.totalCount}</span>
          ) : null}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "services"}
          className={`storage-tab-btn ${tab === "services" ? "active" : ""}`}
          onClick={() => setTab("services")}
        >
          <span>SERVIÇOS</span>
          {services ? (
            <span className="storage-tab-badge">{services.totalCount}</span>
          ) : null}
        </button>
      </nav>

      {tab === "services" && (
        <div className="sh-panel module-dashboard" role="tabpanel">
          <div className="sh-stats">
            <article className="sh-stat">
              <span className="stat-label">EM EXECUÇÃO</span>
              <span className="stat-value">{services ? formatCount(services.runningCount) : "—"}</span>
            </article>
            <article className="sh-stat">
              <span className="stat-label">PARADOS</span>
              <span className="stat-value">{services ? formatCount(services.stoppedCount) : "—"}</span>
            </article>
            <article className="sh-stat">
              <span className="stat-label">PAUSADOS</span>
              <span className="stat-value">{services ? formatCount(services.pausedCount) : "—"}</span>
            </article>
            <article className="sh-stat">
              <span className="stat-label">TOTAL</span>
              <span className="stat-value">{services ? formatCount(services.totalCount) : "—"}</span>
            </article>
          </div>

          <div className="sh-toolbar">
            <input
              type="search"
              className="sh-search"
              placeholder="Buscar por nome ou executável"
              aria-label="Buscar serviços por nome ou executável"
              value={query}
              onChange={event => setQuery(event.target.value)}
            />
            <select
              className="sh-filter"
              aria-label="Filtrar serviços por estado"
              value={filter}
              onChange={event => setFilter(event.target.value as ServiceFilter)}
            >
              <option value="all">Todos</option>
              <option value="running">Em execução</option>
              <option value="stopped">Parados</option>
              <option value="paused">Pausados</option>
            </select>
            <button
              type="button"
              className="btn-primary disk-scan-btn"
              onClick={() => void refreshEverything()}
              disabled={servicesState === "loading"}
            >
              {servicesState === "loading" ? "ATUALIZANDO…" : "ATUALIZAR TUDO"}
            </button>
          </div>

          <div
            className="sh-categories"
            role="group"
            aria-label="Filtrar serviços por origem"
          >
            {CATEGORY_CHIPS.map(chip => {
              const count = chip.value === "all" ? categoryCounts.system + categoryCounts.user + categoryCounts.review + categoryCounts.unknown : categoryCounts[chip.value];
              const selected = category === chip.value;
              return (
                <button
                  key={chip.value}
                  type="button"
                  aria-pressed={selected}
                  className={`sh-chip ${selected ? "active" : ""} sh-chip-${chip.value}`}
                  onClick={() => setCategory(chip.value)}
                >
                  {chip.label}
                  <span className="sh-chip-count">
                    {services ? formatCount(count) : "—"}
                  </span>
                </button>
              );
            })}
          </div>

          {servicesUnavailable && (
            <p className="sh-unavailable" role="alert">
              {services?.unavailableReason}
            </p>
          )}
          {servicesError && (
            <p className="sh-error" role="alert">
              {servicesError}
            </p>
          )}
          {notice && (
            <p className="sh-notice" role="status">
              {notice}
            </p>
          )}

          {servicesState === "loading" && services == null && (
            <div className="pulse-loading sh-loading">CARREGANDO SERVIÇOS…</div>
          )}

          {servicesState === "ready" && services != null && !servicesUnavailable && (
            <div className="sh-list" role="list">
              {visibleServices.length === 0 ? (
                <p className="sh-empty muted">Nenhum serviço corresponde aos filtros aplicados.</p>
              ) : (
                visibleServices.map(service => {
                  const running = service.state === "Em execução";
                  const reasons = service.suspicionReasons ?? [];
                  return (
                    <article
                      className={`sh-item sh-item-${service.category}`}
                      key={service.name}
                      role="listitem"
                    >
                      <img
                        className="sh-item-icon"
                        src={service.icon ?? FALLBACK_ICON}
                        alt=""
                        width={20}
                        height={20}
                      />
                      <div className="sh-item-main">
                        <strong title={serviceLabel(service)}>{serviceLabel(service)}</strong>
                        <small className="sh-item-name">{service.name}</small>
                        <small className="sh-item-path" title={service.executablePath ?? undefined}>
                          {service.executablePath ?? "Executável indisponível"}
                        </small>
                        {reasons.length > 0 && (
                          <ul className="sh-reasons" aria-label="Sinais que exigem revisão">
                            {reasons.map(reason => (
                              <li key={reason}>{reason}</li>
                            ))}
                          </ul>
                        )}
                      </div>
                      <div className="sh-item-meta">
                        <span
                          className={`sh-cat sh-cat-${service.category}`}
                          title={categoryTitle(service)}
                        >
                          {categoryLabel(service.category)}
                        </span>
                        <span
                          className={`status-badge ${running ? "status-enabled" : "status-disabled"}`}
                        >
                          {service.state}
                        </span>
                        <span className="sh-start-type">{service.startType}</span>
                        <small className="sh-item-pid">{formatPid(service)}</small>
                      </div>
                      <button
                        type="button"
                        className="btn-primary disk-scan-btn sh-action"
                        disabled={busyService === service.name}
                        onClick={() => void runServiceAction(service, running ? "stop" : "start")}
                      >
                        {busyService === service.name
                          ? "AGUARDE…"
                          : running
                            ? "PARAR"
                            : "INICIAR"}
                      </button>
                    </article>
                  );
                })
              )}
            </div>
          )}
        </div>
      )}

      {tab === "updates" && (
        <div className="sh-panel module-dashboard" role="tabpanel">
          <div className="sh-stats">
            <article className="sh-stat">
              <span className="stat-label">ATUALIZAÇÕES PENDENTES</span>
              <span className="stat-value">{updates ? formatCount(updates.totalCount) : "—"}</span>
            </article>
          </div>

          <div className="sh-toolbar">
            <button
              type="button"
              className="btn-primary disk-scan-btn"
              onClick={() => void runUpgradeAll()}
              disabled={
                busyAll ||
                updatesState === "loading" ||
                updates == null ||
                updates.totalCount === 0
              }
            >
              {busyAll ? "ATUALIZANDO TUDO…" : "ATUALIZAR TUDO"}
            </button>
          </div>

          {busyAll && (
            <div className="sh-progress-box" role="status">
              <div className="sh-progress-head">
                <span>ATUALIZANDO TODOS OS APLICATIVOS</span>
                <span>{batchElapsed}</span>
              </div>
              <div
                className="sh-progress"
                role="progressbar"
                aria-label="Atualização de todos os aplicativos em andamento"
              >
                <span className="sh-progress-bar sh-progress-indeterminate" />
              </div>
              <small className="sh-progress-log">{batchLog || "Aguardando saída do winget…"}</small>
            </div>
          )}

          {runningCount > 0 && (
            <p className="sh-running-note" role="status">
              {runningCount === 1
                ? "1 atualização em segundo plano"
                : `${formatCount(runningCount)} atualizações em segundo plano`}
            </p>
          )}

          {updatesUnavailable && (
            <p className="sh-unavailable" role="alert">
              {updates?.unavailableReason}
            </p>
          )}
          {updatesError && (
            <p className="sh-error" role="alert">
              {updatesError}
            </p>
          )}
          {notice && (
            <p className="sh-notice" role="status">
              {notice}
            </p>
          )}

          {updatesState === "loading" && updates == null && (
            <div className="pulse-loading sh-loading">CONSULTANDO ATUALIZAÇÕES…</div>
          )}

          {updatesState === "ready" && updates != null && !updatesUnavailable && (
            <div className="sh-list" role="list">
              {updates.updates.length === 0 ? (
                <p className="sh-empty muted">Nenhuma atualização pendente no momento.</p>
              ) : (
                updates.updates.map(update => {
                  const job = updateJobs[update.id];
                  const running = job?.state === "running";
                  const done = job?.state === "done";
                  return (
                    <article
                      className={`sh-item sh-item-update ${running ? "is-running" : ""} ${done ? "is-done" : ""}`}
                      key={`${update.source}:${update.id}`}
                      role="listitem"
                    >
                      <div className="sh-item-main">
                        <strong>{update.name}</strong>
                        <small className="sh-item-name">{update.id}</small>
                        <small className="sh-item-path">Origem: {update.source || "Origem desconhecida"}</small>
                        {running && (
                          <div
                            className="sh-progress sh-progress-inline"
                            role="progressbar"
                            aria-label={`Atualização de ${update.name} em andamento`}
                          >
                            <span className="sh-progress-bar sh-progress-indeterminate" />
                          </div>
                        )}
                        {job?.log && job.state !== "error" && (
                          <small className="sh-progress-log">{job.log}</small>
                        )}
                        {job?.state === "error" && (
                          <small className="sh-item-error" role="alert">
                            {job.message}
                          </small>
                        )}
                      </div>
                      <div className="sh-item-meta">
                        <span className="sh-version">
                          {update.currentVersion || "Indisponível"}
                          <em>→</em>
                          <b>{update.availableVersion || "Indisponível"}</b>
                        </span>
                        {running && job?.startedAt != null && (
                          <small className="sh-item-elapsed">{formatElapsed(now - job.startedAt)}</small>
                        )}
                        {done && <small className="sh-item-elapsed sh-item-elapsed-done">ATUALIZADO</small>}
                      </div>
                      <button
                        type="button"
                        className="btn-primary disk-scan-btn sh-action"
                        disabled={running}
                        onClick={() => void runUpgrade(update)}
                      >
                        {running ? "ATUALIZANDO…" : "ATUALIZAR"}
                      </button>
                    </article>
                  );
                })
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}