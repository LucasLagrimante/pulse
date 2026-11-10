import { invoke } from "@tauri-apps/api/core";
import { Download, Play, RefreshCw, Search, Square } from "lucide-react";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { AI_PROVIDERS, type SupportedAiProvider } from "./AiDiskAdvisorModal";
import { AiProviderCycleButton } from "./AiProviderCycleButton";
import { AiServicesAdvisorModal } from "./AiServicesAdvisorModal";
import "./DataTable.css";
import "./ServicesHub.css";
import {
  clearBatchMessages,
  consumeRefreshRequest,
  getUpgradeJobsState,
  pruneUpgradeJobs,
  startPackageUpgrade,
  startUpgradeAll,
  subscribeUpgradeJobs,
} from "./upgradeJobsStore";

export type { UpdateJob } from "./upgradeJobsStore";

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
  icon?: string | null;
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

export type ServiceSortColumn = "name" | "category" | "state" | "startType";
export type ServiceSortDirection = "asc" | "desc";

const CATEGORY_RANK: Record<string, number> = { review: 0, unknown: 1, user: 2, system: 3 };
const START_TYPE_RANK: Record<string, number> = { "Automático": 0, Manual: 1, Desativado: 2 };

/**
 * Ordena pela coluna do cabeçalho. "Estado" crescente é a ordem padrão de
 * `sortServices` (ativos primeiro); empates caem sempre na ordem alfabética.
 */
export function sortServicesBy(
  services: WindowsService[],
  column: ServiceSortColumn = "state",
  direction: ServiceSortDirection = "asc",
): WindowsService[] {
  const byLabel = (a: WindowsService, b: WindowsService) =>
    serviceLabel(a).localeCompare(serviceLabel(b), "pt-BR", { sensitivity: "base" });
  const rankOf = (service: WindowsService): number => {
    switch (column) {
      case "category":
        return CATEGORY_RANK[service.category] ?? 9;
      case "startType":
        return START_TYPE_RANK[service.startType] ?? 9;
      case "state":
        return STATE_RANK[service.state] ?? 9;
      default:
        return 0;
    }
  };
  const sign = direction === "asc" ? 1 : -1;
  return [...services].sort((a, b) => {
    if (column === "name") return byLabel(a, b) * sign;
    const diff = rankOf(a) - rankOf(b);
    return diff !== 0 ? diff * sign : byLabel(a, b);
  });
}

const SERVICE_SORT_COLUMNS: { id: ServiceSortColumn; label: string; start?: boolean }[] = [
  { id: "name", label: "Serviço", start: true },
  { id: "category", label: "Origem" },
  { id: "state", label: "Estado" },
  { id: "startType", label: "Início" },
];

const FALLBACK_ICON =
  "data:image/svg+xml;utf8," +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" rx="5" fill="rgba(141,169,191,0.18)"/><path d="M12 4v16M4 12h16" stroke="#688099" stroke-width="2.4" stroke-linecap="round"/></svg>',
  );

/* ------------------------------------------------------------------ */
/* Componente                                                          */
/* ------------------------------------------------------------------ */

type LoadState = "idle" | "loading" | "ready";

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

  const [updatesQuery, setUpdatesQuery] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ServiceFilter>("all");
  const [category, setCategory] = useState<ServiceCategoryFilter>("all");
  const [busyService, setBusyService] = useState("");
  const [notice, setNotice] = useState("");
  const [sortBy, setSortBy] = useState<ServiceSortColumn>("state");
  const [sortDirection, setSortDirection] = useState<ServiceSortDirection>("asc");

  const [showAiModal, setShowAiModal] = useState(false);
  const [aiModalProvider, setAiModalProvider] = useState<SupportedAiProvider>("antigravity");
  const [availableAiProviders, setAvailableAiProviders] = useState<SupportedAiProvider[] | null>(null);

  // Instalações vivem fora do componente: o hub é desmontado ao trocar de área,
  // mas o winget segue rodando e o progresso precisa estar lá na volta.
  const upgrades = useSyncExternalStore(subscribeUpgradeJobs, getUpgradeJobsState);
  const { jobs: updateJobs, busyAll, batchLog, batchStartedAt, batchNotice, batchError, needsRefresh } =
    upgrades;

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

  // Um job que não está mais na lista de pendências já cumpriu seu papel.
  useEffect(() => {
    if (updates == null) return;
    pruneUpgradeJobs(new Set(updates.updates.map(update => update.id)));
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

  // Instalação concluída (com o hub aberto ou em segundo plano): a lista de
  // pendências está defasada e é reconsultada sem cache.
  useEffect(() => {
    if (needsRefresh && consumeRefreshRequest()) void loadUpdates(true);
  }, [needsRefresh, loadUpdates]);

  const categoryCounts = useMemo(
    () => countByCategory(services?.services ?? []),
    [services],
  );

  const visibleServices = useMemo(
    () => sortServicesBy(filterServices(services?.services ?? [], query, filter, category), sortBy, sortDirection),
    [services, query, filter, category, sortBy, sortDirection],
  );

  const handleSort = (column: ServiceSortColumn) => {
    if (column === sortBy) {
      setSortDirection(current => (current === "asc" ? "desc" : "asc"));
      return;
    }
    setSortBy(column);
    setSortDirection("asc");
  };

  const hasServicesForAi = tab === "services" && services != null && !services.unavailableReason;

  // Descobre os CLIs de IA só quando o botão vai aparecer (o backend guarda em cache).
  useEffect(() => {
    if (!hasServicesForAi || availableAiProviders != null) return;
    let active = true;
    invoke<string[]>("detect_ai_cli_providers")
      .then(ids => {
        if (active && Array.isArray(ids)) {
          setAvailableAiProviders(ids.filter((id): id is SupportedAiProvider => AI_PROVIDERS.some(p => p.id === id)));
        }
      })
      .catch(() => {
        // Sem detecção o botão segue alternando entre os três suportados
      });
    return () => {
      active = false;
    };
  }, [hasServicesForAi, availableAiProviders]);

  const openAiModal = (provider: SupportedAiProvider) => {
    setAiModalProvider(provider);
    setShowAiModal(true);
  };

  const visibleUpdates = useMemo(() => {
    if (!updates) return [];
    const q = updatesQuery.trim().toLowerCase();
    if (!q) return updates.updates;
    return updates.updates.filter(
      u =>
        u.name.toLowerCase().includes(q) ||
        u.id.toLowerCase().includes(q) ||
        u.source.toLowerCase().includes(q) ||
        u.currentVersion.toLowerCase().includes(q) ||
        u.availableVersion.toLowerCase().includes(q),
    );
  }, [updates, updatesQuery]);

  const updateSourcesLabel = useMemo(() => {
    if (!updates || updates.updates.length === 0) return "Repositório winget";
    const sources = Array.from(new Set(updates.updates.map(u => u.source).filter(Boolean)));
    return sources.length > 0 ? sources.join(", ") : "winget oficial";
  }, [updates]);

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

  const runUpgrade = (update: PendingUpdate) => startPackageUpgrade(update.id);

  const runUpgradeAll = () => {
    setUpdatesError("");
    setNotice("");
    return startUpgradeAll();
  };

  /**
   * "Atualizar tudo" na aba de Serviços reconsulta os dois inventários:
   * serviços e atualizações pendentes. Nada é iniciado/parado em lote —
   * mudança de estado em massa é destrutiva e depende de elevação.
   */
  const refreshEverything = async () => {
    setNotice("");
    clearBatchMessages();
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

  const skeletonRows = (prefix: string) => (
    <ul className="dt-list sh-skeleton-list" aria-busy="true" aria-label={`Carregando lista de ${prefix}`}>
      {Array.from({ length: 6 }).map((_, i) => (
        <li className="dt-row sh-skeleton-row" key={`${prefix}-skeleton-${i}`} aria-hidden="true">
          <div className="dt-identity">
            <span className="sh-skeleton-box sh-skeleton-icon" />
            <span className="dt-name">
              <span className="sh-skeleton-box sh-skeleton-title" />
              <span className="sh-skeleton-box sh-skeleton-sub" />
            </span>
          </div>
          <span className="sh-skeleton-box sh-skeleton-badge" />
          <span className="sh-skeleton-box sh-skeleton-status" />
          <span className="sh-skeleton-box sh-skeleton-pid" />
          <span className="sh-skeleton-box sh-skeleton-btn" />
        </li>
      ))}
    </ul>
  );

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
        <div className="dt dt--services" role="tabpanel">
          <div className="dt-toolbar">
            <div className="dt-toolbar-start">
              <label className="dt-search">
                <Search size={15} aria-hidden="true" />
                <input
                  type="search"
                  placeholder="Buscar por nome ou executável"
                  aria-label="Buscar serviços por nome ou executável"
                  value={query}
                  onChange={event => setQuery(event.target.value)}
                />
              </label>
              <select
                className="sh-filter"
                aria-label="Filtrar serviços por estado"
                value={filter}
                onChange={event => setFilter(event.target.value as ServiceFilter)}
              >
                <option value="all">Todos os estados</option>
                <option value="running">Em execução</option>
                <option value="stopped">Parados</option>
                <option value="paused">Pausados</option>
              </select>
              <button
                type="button"
                className="sh-btn"
                onClick={() => void refreshEverything()}
                disabled={servicesState === "loading"}
              >
                <RefreshCw size={13} aria-hidden="true" />
                {servicesState === "loading" ? "ATUALIZANDO…" : "ATUALIZAR TUDO"}
              </button>
            </div>
            <dl className="dt-summary" aria-label="Resumo dos serviços">
              <div>
                <dt>Em execução</dt>
                <dd>{services ? formatCount(services.runningCount) : "—"}</dd>
              </div>
              <div>
                <dt>Parados</dt>
                <dd>{services ? formatCount(services.stoppedCount) : "—"}</dd>
              </div>
              <div>
                <dt>Pausados</dt>
                <dd>{services ? formatCount(services.pausedCount) : "—"}</dd>
              </div>
              <div>
                <dt>Total</dt>
                <dd>{services ? formatCount(services.totalCount) : "—"}</dd>
              </div>
            </dl>
          </div>

          <div className="sh-filter-row">
            <div className="sh-categories" role="group" aria-label="Filtrar serviços por origem">
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
            {hasServicesForAi && (
              <AiProviderCycleButton
                className="disk-ai-advisor-trigger-btn sh-ai-trigger"
                available={availableAiProviders}
                prefix="REVISAR SEGURANÇA COM"
                suffix="NO TERMINAL"
                purpose="para revisar os serviços em execução em busca de keylogger, spyware ou acesso indevido"
                onOpen={openAiModal}
              />
            )}
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
            <div className="sh-loading-banner" role="status" aria-live="polite">
              <span className="sh-loading-spinner" aria-hidden="true" />
              <div className="sh-loading-text">
                <strong>Consultando Gerenciador de Controle de Serviços (SCM)…</strong>
                <small>Classificando serviços por conta de execução, diretório e integridade</small>
              </div>
            </div>
          )}

          {!servicesUnavailable && (
            <div className="dt-table">
              <div className="dt-head">
                {SERVICE_SORT_COLUMNS.map(({ id, label, start }) => {
                  const isActive = sortBy === id;
                  return (
                    <button
                      key={id}
                      type="button"
                      className={`dt-sort${start ? " is-start" : ""}${isActive ? " is-active" : ""}`}
                      aria-label={`Ordenar por ${label.toLowerCase()}${isActive ? (sortDirection === "desc" ? ", decrescente" : ", crescente") : ""}`}
                      aria-pressed={isActive}
                      onClick={() => handleSort(id)}
                    >
                      {label}
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

              {servicesState === "loading" && services == null ? (
                skeletonRows("serviços")
              ) : services == null ? null : visibleServices.length === 0 ? (
                <div className="dt-empty" role="status">
                  <p>Nenhum serviço corresponde aos filtros aplicados.</p>
                  {query.trim() && (
                    <button type="button" className="dt-empty-reset" onClick={() => setQuery("")}>
                      Limpar busca
                    </button>
                  )}
                </div>
              ) : (
                <ul className="dt-list" role="list">
                  {visibleServices.map(service => {
                    const running = service.state === "Em execução";
                    const busy = busyService === service.name;
                    const reasons = service.suspicionReasons ?? [];
                    const label = serviceLabel(service);
                    return (
                      <li
                        className={`dt-row row-hl sh-row sh-row-${service.category}${busy ? " is-busy" : ""}`}
                        key={service.name}
                      >
                        <div className="dt-identity" title={[label, service.executablePath].filter(Boolean).join(" · ")}>
                          <span className="dt-icon" aria-hidden="true">
                            <img src={service.icon ?? FALLBACK_ICON} alt="" />
                          </span>
                          <span className="dt-name">
                            <strong>{label}</strong>
                            <small>
                              <span>{service.name}</span> · {service.executablePath ?? "Executável indisponível"}
                            </small>
                            {reasons.length > 0 && (
                              <ul className="sh-reasons" aria-label="Sinais que exigem revisão">
                                {reasons.map(reason => (
                                  <li key={reason}>{reason}</li>
                                ))}
                              </ul>
                            )}
                          </span>
                        </div>
                        <span className="dt-cell sh-cell-end">
                          <span className={`sh-cat sh-cat-${service.category}`} title={categoryTitle(service)}>
                            {categoryLabel(service.category)}
                          </span>
                        </span>
                        <span className="sh-state-cell">
                          <span className={`sh-state ${running ? "is-running" : ""}`}>{service.state}</span>
                          <small className="sh-item-pid">{formatPid(service)}</small>
                        </span>
                        <span className="dt-cell sh-cell-end">{service.startType}</span>
                        <span className="dt-action">
                          <button
                            type="button"
                            className={`row-action${running ? " row-hl-danger" : ""}`}
                            aria-label={`${running ? "Parar" : "Iniciar"} ${label}`}
                            title={`${running ? "Parar" : "Iniciar"} o serviço ${label}`}
                            disabled={busy}
                            onClick={() => void runServiceAction(service, running ? "stop" : "start")}
                          >
                            {running ? (
                              <Square size={12} strokeWidth={2.2} aria-hidden="true" />
                            ) : (
                              <Play size={13} strokeWidth={2.2} aria-hidden="true" />
                            )}
                            <span>{busy ? "Aguarde…" : running ? "Parar" : "Iniciar"}</span>
                          </button>
                        </span>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          )}
        </div>
      )}

      {tab === "updates" && (
        <div className="dt dt--updates" role="tabpanel">
          <div className="dt-toolbar">
            <div className="dt-toolbar-start">
              <label className="dt-search">
                <Search size={15} aria-hidden="true" />
                <input
                  type="search"
                  placeholder="Buscar aplicativo (nome, ID ou versão)"
                  aria-label="Buscar aplicativo por nome, ID ou versão"
                  value={updatesQuery}
                  onChange={event => setUpdatesQuery(event.target.value)}
                  disabled={updatesState === "loading" && updates == null}
                />
              </label>
              <div className="sh-toolbar-actions">
                <button
                  type="button"
                  className="sh-btn"
                  onClick={() => void loadUpdates(true)}
                  disabled={updatesState === "loading" || busyAll}
                  title="Reconsultar atualizações disponíveis no winget"
                >
                  <RefreshCw size={13} aria-hidden="true" />
                  {updatesState === "loading" ? "VERIFICANDO…" : "VERIFICAR"}
                </button>
                <button
                  type="button"
                  className="sh-btn sh-btn-primary"
                  onClick={() => void runUpgradeAll()}
                  disabled={
                    busyAll ||
                    updatesState === "loading" ||
                    updates == null ||
                    updates.totalCount === 0
                  }
                >
                  <Download size={13} aria-hidden="true" />
                  {busyAll ? "ATUALIZANDO TUDO…" : "ATUALIZAR TUDO"}
                </button>
              </div>
            </div>
            <dl className="dt-summary" aria-label="Resumo das atualizações">
              <div>
                <dt>Pendentes</dt>
                <dd>{updates ? formatCount(updates.totalCount) : "—"}</dd>
              </div>
              <div>
                <dt>Em segundo plano</dt>
                <dd>{busyAll ? "1 em lote" : formatCount(runningCount)}</dd>
              </div>
              <div>
                <dt>Repositório</dt>
                <dd className="sh-summary-text" title={updates ? updateSourcesLabel : undefined}>
                  {updates ? updateSourcesLabel : "—"}
                </dd>
              </div>
            </dl>
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
          {(updatesError || batchError) && (
            <p className="sh-error" role="alert">
              {updatesError || batchError}
            </p>
          )}
          {(notice || batchNotice) && (
            <p className="sh-notice" role="status">
              {notice || batchNotice}
            </p>
          )}

          {updatesState === "loading" && updates == null && (
            <div className="sh-loading-banner" role="status" aria-live="polite">
              <span className="sh-loading-spinner" aria-hidden="true" />
              <div className="sh-loading-text">
                <strong>Consultando repositório oficial do Gerenciador de Pacotes do Windows (winget)…</strong>
                <small>Comparando versões instaladas com manifestos recentes do repositório</small>
              </div>
            </div>
          )}

          {!updatesUnavailable && (
            <div className="dt-table">
              <div className="dt-head">
                <span>Aplicativo</span>
                <span className="sh-head-right">Instalada</span>
                <span className="sh-head-right">Disponível</span>
                <span className="dt-center" aria-hidden="true">Ação</span>
              </div>

              {updatesState === "loading" && updates == null ? (
                skeletonRows("atualizações")
              ) : updates == null ? null : updates.updates.length === 0 ? (
                <div className="dt-empty" role="status">
                  <p>Nenhuma atualização pendente no momento.</p>
                </div>
              ) : visibleUpdates.length === 0 ? (
                <div className="dt-empty" role="status">
                  <p>Nenhum aplicativo corresponde aos filtros aplicados.</p>
                  <button type="button" className="dt-empty-reset" onClick={() => setUpdatesQuery("")}>
                    Limpar busca
                  </button>
                </div>
              ) : (
                <ul className="dt-list" role="list">
                  {visibleUpdates.map(update => {
                    const job = updateJobs[update.id];
                    const running = job?.state === "running";
                    const done = job?.state === "done";
                    return (
                      <li
                        className={`dt-row row-hl sh-row-update${running ? " is-running" : ""}${done ? " is-done" : ""}`}
                        key={`${update.source}:${update.id}`}
                      >
                        <div className="dt-identity">
                          <span className="dt-icon" aria-hidden="true">
                            {update.icon ? (
                              <img src={update.icon} alt="" />
                            ) : (
                              update.name.slice(0, 2).toUpperCase()
                            )}
                          </span>
                          <span className="dt-name">
                            <strong>{update.name}</strong>
                            <small>
                              <span>{update.id}</span> · <span>Origem: {update.source || "Origem desconhecida"}</span>
                            </small>
                            {running && (
                              <span
                                className="sh-progress sh-progress-inline"
                                role="progressbar"
                                aria-label={`Atualização de ${update.name} em andamento`}
                              >
                                <span className="sh-progress-bar sh-progress-indeterminate" />
                              </span>
                            )}
                            {job?.log && job.state !== "error" && (
                              <small className="sh-progress-log">{job.log}</small>
                            )}
                            {job?.state === "error" && (
                              <small className="sh-item-error" role="alert">
                                {job.message}
                              </small>
                            )}
                          </span>
                        </div>
                        <span className={`dt-cell is-numeric${update.currentVersion ? "" : " is-missing"}`}>
                          {update.currentVersion || "Indisponível"}
                        </span>
                        <span className="sh-version-cell">
                          <b className="sh-version-new">{update.availableVersion || "Indisponível"}</b>
                          {running && job?.startedAt != null && (
                            <small className="sh-item-elapsed">{formatElapsed(now - job.startedAt)}</small>
                          )}
                          {done && <small className="sh-item-elapsed sh-item-elapsed-done">ATUALIZADO</small>}
                        </span>
                        <span className="dt-action">
                          <button
                            type="button"
                            className="row-action"
                            aria-label={`Atualizar ${update.name}`}
                            title={`Atualizar ${update.name} para ${update.availableVersion || "a versão disponível"} via winget`}
                            disabled={running}
                            onClick={() => void runUpgrade(update)}
                          >
                            <Download size={13} strokeWidth={2.1} aria-hidden="true" />
                            <span>{running ? "Atualizando…" : "Atualizar"}</span>
                          </button>
                        </span>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          )}
        </div>
      )}

      <AiServicesAdvisorModal
        isOpen={showAiModal}
        onClose={() => setShowAiModal(false)}
        initialProvider={aiModalProvider}
      />
    </section>
  );
}
