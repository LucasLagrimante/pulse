import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Activity,
  AppWindow,
  Bug,
  Calculator,
  Check,
  Clock,
  CornerDownLeft,
  Cpu,
  FileText,
  FlaskConical,
  Folder,
  HardDrive,
  LayoutGrid,
  Lock,
  Moon,
  PackageMinus,
  Power,
  RefreshCw,
  RotateCw,
  Search,
  Settings,
  Shield,
  Sparkles,
  Terminal,
  Trash2,
  Wifi,
  Wrench,
  X,
} from "lucide-react";
import { PulseMark } from "./Identity";
import "./LauncherWindow.css";

export type LauncherCategory = "all" | "apps" | "files" | "pulse" | "system";

export interface LauncherItem {
  id: string;
  kind: "calc" | "app" | "module" | "system" | "file" | "folder" | string;
  title: string;
  subtitle: string;
  target: string;
  category: string;
  badge: string;
  iconHint: string;
  /** Data URI PNG do ícone real do app; ausente cai para o glifo semântico. */
  icon?: string | null;
  actionLabel?: string | null;
  isRecent: boolean;
  score: number;
}

export interface LauncherCalcResult {
  expression: string;
  formatted: string;
}

export interface LauncherResponse {
  query: string;
  category: string;
  calc: LauncherCalcResult | null;
  recent: LauncherItem[];
  items: LauncherItem[];
  indexedAppCount: number;
  isIndexing: boolean;
  searchElapsedMs: number;
}

export interface LauncherActionResult {
  ok: boolean;
  action: string;
  message: string;
  closeLauncher: boolean;
}

interface NavigableEntry {
  key: string;
  item: LauncherItem;
  isCalcCard?: boolean;
}

const CATEGORIES: { id: LauncherCategory; label: string }[] = [
  { id: "all", label: "Tudo" },
  { id: "apps", label: "Apps" },
  { id: "files", label: "Arquivos" },
  { id: "pulse", label: "Pulse" },
  { id: "system", label: "Sistema" },
];

const CALC_EXAMPLES = ["1280 * 0.85", "(45 + 15) / 3", "sqrt(144)", "2pi"];

const EMPTY_RESPONSE: LauncherResponse = {
  query: "",
  category: "all",
  calc: null,
  recent: [],
  items: [],
  indexedAppCount: 0,
  isIndexing: false,
  searchElapsedMs: 0,
};

function sanitizeCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.trunc(value)
    : 0;
}

function renderItemIcon(item: LauncherItem) {
  if (item.icon) {
    return <img className="launcher-row-app-icon" src={item.icon} alt="" aria-hidden />;
  }
  const hint = item.iconHint || item.kind;
  const props = { size: 17, strokeWidth: 1.9, "aria-hidden": true as const };

  switch (hint) {
    case "calc":
      return <Calculator {...props} />;
    case "pulse:overview":
      return <LayoutGrid {...props} />;
    case "pulse:activity":
      return <Activity {...props} />;
    case "pulse:performance":
      return <Cpu {...props} />;
    case "pulse:files":
      return <Folder {...props} />;
    case "pulse:storage":
      return <HardDrive {...props} />;
    case "pulse:cleanup":
      return <Trash2 {...props} />;
    case "pulse:services":
      return <Wrench {...props} />;
    case "pulse:uninstall":
      return <PackageMinus {...props} />;
    case "pulse:quotas":
      return <Sparkles {...props} />;
    case "pulse:startup":
      return <Shield {...props} />;
    case "pulse:security":
      return <Lock {...props} />;
    case "pulse:network":
      return <Wifi {...props} />;
    case "pulse:beta":
      return <FlaskConical {...props} />;
    case "pulse:debug":
      return <Bug {...props} />;
    case "system:lock":
      return <Lock {...props} />;
    case "system:sleep":
      return <Moon {...props} />;
    case "system:restart":
      return <RotateCw {...props} />;
    case "system:shutdown":
      return <Power {...props} />;
    case "system:settings":
      return <Settings {...props} />;
    case "system:terminal":
      return <Terminal {...props} />;
    case "system:taskmgr":
      return <Activity {...props} />;
    case "folder":
      return <Folder {...props} />;
    case "file":
      return <FileText {...props} />;
    default:
      return <AppWindow {...props} />;
  }
}

function iconToneClass(item: LauncherItem): string {
  if (item.kind === "module") return "tone-pulse";
  if (item.kind === "system") {
    if (item.target === "system:shutdown" || item.target === "system:restart") {
      return "tone-danger";
    }
    return "tone-system";
  }
  if (item.kind === "folder" || item.kind === "file") return "tone-file";
  if (item.kind === "calc") return "tone-calc";
  return "tone-app";
}

function emptySectionTitle(category: LauncherCategory): string {
  switch (category) {
    case "apps":
      return "Aplicativos instalados";
    case "pulse":
      return "Módulos do Pulse";
    case "system":
      return "Ações rápidas do sistema";
    case "files":
      return "Busca de arquivos locais";
    default:
      return "Acesso rápido do Pulse";
  }
}

/** Máximo de respostas guardadas no cache em memória do runner. */
const RESPONSE_CACHE_LIMIT = 60;

function responseCacheKey(query: string, category: LauncherCategory): string {
  return `${category}\u0000${query.trim().toLowerCase()}`;
}

export function LauncherWindow() {
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<LauncherCategory>("all");
  const [response, setResponse] = useState<LauncherResponse>(EMPTY_RESPONSE);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [copiedText, setCopiedText] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [refreshingIndex, setRefreshingIndex] = useState(false);

  const inputRef = useRef<HTMLInputElement | null>(null);
  const rowRefs = useRef<Map<number, HTMLElement>>(new Map());
  const requestSeqRef = useRef(0);
  const copiedTimerRef = useRef<number | null>(null);

  // Mantém o tema sincronizado com a preferência salva pelo app principal.
  useEffect(() => {
    const syncThemeFromStorage = () => {
      try {
        const stored = window.localStorage.getItem("pulse-theme");
        if (stored === "dark" || stored === "light") {
          document.documentElement.dataset.theme = stored;
        }
      } catch {
        // Ignora indisponibilidade de storage em ambientes restritos.
      }
    };
    syncThemeFromStorage();
    window.addEventListener("storage", syncThemeFromStorage);
    return () => window.removeEventListener("storage", syncThemeFromStorage);
  }, []);

  // Cache em memória das respostas (stale-while-revalidate): a resposta
  // guardada aparece na hora e a consulta ao backend só a atualiza.
  const responseCacheRef = useRef<Map<string, LauncherResponse>>(new Map());

  const runQuery = useCallback(async (nextQuery: string, nextCategory: LauncherCategory) => {
    const seq = ++requestSeqRef.current;
    const cacheKey = responseCacheKey(nextQuery, nextCategory);
    const cache = responseCacheRef.current;
    const cached = cache.get(cacheKey);
    if (cached) {
      setResponse(cached);
      setSelectedIndex(0);
    }
    try {
      const data = await invoke<LauncherResponse>("launcher_query", {
        query: nextQuery,
        category: nextCategory,
      });
      if (seq !== requestSeqRef.current) return;
      if (data != null && typeof data === "object") {
        const fresh: LauncherResponse = {
          query: typeof data.query === "string" ? data.query : nextQuery,
          category: typeof data.category === "string" ? data.category : nextCategory,
          calc: data.calc ?? null,
          recent: Array.isArray(data.recent) ? data.recent : [],
          items: Array.isArray(data.items) ? data.items : [],
          indexedAppCount: sanitizeCount(data.indexedAppCount),
          isIndexing: Boolean(data.isIndexing),
          searchElapsedMs: sanitizeCount(data.searchElapsedMs),
        };
        // Reinsere no fim para manter a ordem de uso (LRU).
        cache.delete(cacheKey);
        cache.set(cacheKey, fresh);
        if (cache.size > RESPONSE_CACHE_LIMIT) {
          const oldest = cache.keys().next().value;
          if (oldest !== undefined) cache.delete(oldest);
        }
        setResponse(fresh);
        if (!cached) setSelectedIndex(0);
        setStatusError(null);
      }
    } catch (error) {
      if (seq !== requestSeqRef.current) return;
      const detail = error instanceof Error ? error.message : String(error);
      setStatusError(detail || "Falha ao consultar o índice do Pulse Runner.");
    }
  }, []);

  // Consulta imediata quando a busca está vazia ou muda de aba; debounce curto ao digitar.
  useEffect(() => {
    const delay = query.trim().length === 0 ? 0 : 30;
    const timer = window.setTimeout(() => {
      void runQuery(query, category);
    }, delay);
    return () => window.clearTimeout(timer);
  }, [query, category, runQuery]);

  // Volta ao estado inicial (busca vazia, aba "Tudo"). Roda ao esconder a
  // janela, para a próxima abertura já surgir zerada, e de novo ao abrir.
  const resetToIdle = useCallback(() => {
    setQuery("");
    setCategory("all");
    setSelectedIndex(0);
    setCopiedText(null);
    setStatusError(null);
    void runQuery("", "all");
  }, [runQuery]);

  // Foca o input na montagem e sempre que a janela é reaberta via tecla Windows.
  useEffect(() => {
    inputRef.current?.focus();
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const track = (pending: Promise<() => void>) => {
      pending
        .then((cleanup) => {
          if (disposed) cleanup();
          else unlisteners.push(cleanup);
        })
        .catch(() => undefined);
    };

    track(
      listen("pulse://launcher-opened", () => {
        if (disposed) return;
        resetToIdle();
        window.requestAnimationFrame(() => {
          inputRef.current?.focus();
          inputRef.current?.select();
        });
      }),
    );
    track(
      listen<boolean>("pulse://foreground-changed", (event) => {
        if (disposed || event.payload !== false) return;
        resetToIdle();
      }),
    );

    return () => {
      disposed = true;
      unlisteners.forEach((cleanup) => cleanup());
      if (copiedTimerRef.current != null) {
        window.clearTimeout(copiedTimerRef.current);
      }
    };
  }, [resetToIdle]);

  // Lista unificada e ordenada de todos os alvos selecionáveis pelo teclado.
  const entries = useMemo<NavigableEntry[]>(() => {
    const list: NavigableEntry[] = [];
    if (response.calc) {
      list.push({
        key: "calc:hero",
        isCalcCard: true,
        item: {
          id: "calc:result",
          kind: "calc",
          title: response.calc.formatted,
          subtitle: response.calc.expression,
          target: response.calc.formatted,
          category: "calc",
          badge: "Cálculo",
          iconHint: "calc",
          actionLabel: "Copiar",
          isRecent: false,
          score: 10_000,
        },
      });
    }

    if (query.trim().length === 0) {
      for (const recentItem of response.recent) {
        list.push({
          key: `recent:${recentItem.id}`,
          item: recentItem,
        });
      }
    }

    for (const item of response.items) {
      list.push({
        key: `item:${item.id}`,
        item,
      });
    }

    return list;
  }, [query, response.calc, response.recent, response.items]);

  // Mantém índice dentro dos limites quando a lista muda.
  const safeSelectedIndex = entries.length > 0 ? Math.min(selectedIndex, entries.length - 1) : -1;

  useEffect(() => {
    if (safeSelectedIndex < 0) return;
    const node = rowRefs.current.get(safeSelectedIndex);
    if (node && typeof node.scrollIntoView === "function") {
      node.scrollIntoView({ block: "nearest" });
    }
  }, [safeSelectedIndex]);

  const executeEntry = useCallback(async (entry: NavigableEntry) => {
    const { item } = entry;
    setStatusError(null);

    if (item.kind === "calc") {
      if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
        void navigator.clipboard.writeText(item.target).catch(() => undefined);
      }
      setCopiedText(item.target);
      if (copiedTimerRef.current != null) {
        window.clearTimeout(copiedTimerRef.current);
      }
      copiedTimerRef.current = window.setTimeout(() => {
        setCopiedText(null);
      }, 1800);
    }

    try {
      await invoke<LauncherActionResult>("launcher_execute", {
        itemId: item.id,
        kind: item.kind,
        target: item.target,
        title: item.title,
      });
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      setStatusError(detail || "Não foi possível executar o item selecionado.");
    }
  }, []);

  const handleHide = useCallback(() => {
    void invoke("launcher_hide").catch(() => undefined);
  }, []);

  const handleRefreshIndex = useCallback(async () => {
    if (refreshingIndex) return;
    setRefreshingIndex(true);
    setStatusError(null);
    try {
      await invoke<number>("launcher_refresh_index");
      await runQuery(query, category);
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      setStatusError(detail || "Não foi possível atualizar o índice de aplicativos.");
    } finally {
      setRefreshingIndex(false);
      inputRef.current?.focus();
    }
  }, [category, query, refreshingIndex, runQuery]);

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        handleHide();
        return;
      }

      if (event.key === "Tab") {
        event.preventDefault();
        const currentPos = CATEGORIES.findIndex((c) => c.id === category);
        const delta = event.shiftKey ? -1 : 1;
        const nextPos = (currentPos + delta + CATEGORIES.length) % CATEGORIES.length;
        setCategory(CATEGORIES[nextPos].id);
        setSelectedIndex(0);
        inputRef.current?.focus();
        return;
      }

      if (event.key === "ArrowDown") {
        event.preventDefault();
        if (entries.length === 0) return;
        setSelectedIndex((prev) => (prev + 1) % entries.length);
        return;
      }

      if (event.key === "ArrowUp") {
        event.preventDefault();
        if (entries.length === 0) return;
        setSelectedIndex((prev) => (prev - 1 + entries.length) % entries.length);
        return;
      }

      if (event.key === "Enter") {
        event.preventDefault();
        if (safeSelectedIndex >= 0 && entries[safeSelectedIndex]) {
          void executeEntry(entries[safeSelectedIndex]);
        }
      }
    },
    [category, entries, executeEntry, handleHide, safeSelectedIndex],
  );

  const isEmptyQuery = query.trim().length === 0;
  const calcOffset = response.calc ? 1 : 0;
  const recentCount = isEmptyQuery ? response.recent.length : 0;
  const itemsStartIndex = calcOffset + recentCount;

  const setRowRef = (index: number) => (el: HTMLElement | null) => {
    if (el) rowRefs.current.set(index, el);
    else rowRefs.current.delete(index);
  };

  return (
    <main
      className="launcher-shell"
      aria-label="Pulse Runner"
      onKeyDown={handleKeyDown}
    >
      <header className="launcher-header">
        <div className="launcher-search-bar">
          <div className="launcher-brand-slot" aria-hidden="true">
            {isEmptyQuery ? (
              <PulseMark />
            ) : (
              <Search className="launcher-search-glyph" size={20} strokeWidth={2.1} />
            )}
          </div>

          <input
            ref={inputRef}
            type="text"
            className="launcher-input"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Buscar apps, arquivos, módulos do Pulse ou calcular…"
            aria-label="Buscar no Pulse Runner"
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
          />

          {!isEmptyQuery && (
            <button
              type="button"
              className="launcher-clear-btn"
              onClick={() => {
                setQuery("");
                setSelectedIndex(0);
                inputRef.current?.focus();
              }}
              aria-label="Limpar busca"
              title="Limpar busca"
            >
              <X size={14} strokeWidth={2.2} />
            </button>
          )}

          <div className="launcher-shortcut-badges" aria-hidden="true">
            <kbd className="launcher-kbd launcher-kbd-accent">Win</kbd>
            <kbd className="launcher-kbd">Esc</kbd>
          </div>
        </div>

        <nav className="launcher-chips" aria-label="Filtrar resultados por categoria">
          {CATEGORIES.map((chip) => {
            const active = category === chip.id;
            return (
              <button
                key={chip.id}
                type="button"
                role="tab"
                aria-selected={active}
                className={`launcher-chip${active ? " is-active" : ""}`}
                onClick={() => {
                  setCategory(chip.id);
                  setSelectedIndex(0);
                  inputRef.current?.focus();
                }}
              >
                {chip.label}
              </button>
            );
          })}
          <span className="launcher-tab-hint" aria-hidden="true">
            <kbd className="launcher-kbd">Tab</kbd>
          </span>
        </nav>
      </header>

      {statusError && (
        <div className="launcher-alert" role="alert">
          <span>{statusError}</span>
          <button
            type="button"
            className="launcher-alert-close"
            onClick={() => setStatusError(null)}
            aria-label="Fechar aviso"
          >
            <X size={13} />
          </button>
        </div>
      )}

      <section className="launcher-body" aria-label="Resultados do Pulse Runner">
        {/* Cartão destacado de Calculadora Instantânea */}
        {response.calc && (
          <div className="launcher-section">
            <div className="launcher-section-label">
              <Calculator size={13} aria-hidden="true" />
              <span>Calculadora instantânea</span>
            </div>
            <button
              ref={setRowRef(0)}
              type="button"
              className={`launcher-calc-card${safeSelectedIndex === 0 ? " is-selected" : ""}`}
              onMouseEnter={() => setSelectedIndex(0)}
              onClick={() => {
                if (entries[0]) void executeEntry(entries[0]);
              }}
            >
              <div className="launcher-calc-expr">
                <span className="launcher-calc-tag">=</span>
                <code>{response.calc.expression}</code>
              </div>
              <div className="launcher-calc-value-row">
                <strong className="launcher-calc-result">{response.calc.formatted}</strong>
                <span
                  className={`launcher-calc-copy-pill${
                    copiedText === response.calc.formatted ? " is-copied" : ""
                  }`}
                >
                  {copiedText === response.calc.formatted ? (
                    <>
                      <Check size={13} strokeWidth={2.4} />
                      <span>Copiado!</span>
                    </>
                  ) : (
                    <>
                      <span>Copiar</span>
                      <kbd className="launcher-kbd launcher-kbd-inline">↵</kbd>
                    </>
                  )}
                </span>
              </div>
            </button>
          </div>
        )}

        {/* Seção Recentes quando a busca está vazia */}
        {isEmptyQuery && response.recent.length > 0 && (
          <div className="launcher-section">
            <div className="launcher-section-label">
              <Clock size={13} aria-hidden="true" />
              <span>Recentes</span>
            </div>
            <ul className="launcher-list" role="listbox" aria-label="Itens recentes">
              {response.recent.map((item, idx) => {
                const flatIndex = calcOffset + idx;
                const selected = safeSelectedIndex === flatIndex;
                return (
                  <li key={`recent:${item.id}`} role="option" aria-selected={selected}>
                    <button
                      ref={setRowRef(flatIndex)}
                      type="button"
                      className={`launcher-row${selected ? " is-selected" : ""}`}
                      onMouseEnter={() => setSelectedIndex(flatIndex)}
                      onClick={() => {
                        const entry = entries[flatIndex];
                        if (entry) void executeEntry(entry);
                      }}
                    >
                      <span className={`launcher-row-icon ${iconToneClass(item)}`}>
                        {renderItemIcon(item)}
                      </span>
                      <span className="launcher-row-text">
                        <span className="launcher-row-title">{item.title}</span>
                        <span className="launcher-row-subtitle">{item.subtitle}</span>
                      </span>
                      <span className="launcher-row-meta">
                        <span className="launcher-badge">{item.badge}</span>
                        {selected && (
                          <span className="launcher-enter-hint">
                            <span>{item.actionLabel || "Abrir"}</span>
                            <CornerDownLeft size={12} aria-hidden="true" />
                          </span>
                        )}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        {/* Lista principal (acesso rápido vazio ou resultados ranqueados) */}
        {response.items.length > 0 && (
          <div className="launcher-section">
            <div className="launcher-section-label">
              <span>
                {isEmptyQuery
                  ? emptySectionTitle(category)
                  : `Resultados (${response.items.length})`}
              </span>
            </div>
            <ul className="launcher-list" role="listbox" aria-label="Resultados encontrados">
              {response.items.map((item, idx) => {
                const flatIndex = itemsStartIndex + idx;
                const selected = safeSelectedIndex === flatIndex;
                return (
                  <li key={`item:${item.id}`} role="option" aria-selected={selected}>
                    <button
                      ref={setRowRef(flatIndex)}
                      type="button"
                      className={`launcher-row${selected ? " is-selected" : ""}`}
                      onMouseEnter={() => setSelectedIndex(flatIndex)}
                      onClick={() => {
                        const entry = entries[flatIndex];
                        if (entry) void executeEntry(entry);
                      }}
                    >
                      <span className={`launcher-row-icon ${iconToneClass(item)}`}>
                        {renderItemIcon(item)}
                      </span>
                      <span className="launcher-row-text">
                        <span className="launcher-row-title">{item.title}</span>
                        <span className="launcher-row-subtitle">{item.subtitle}</span>
                      </span>
                      <span className="launcher-row-meta">
                        <span className="launcher-badge">{item.badge}</span>
                        {selected && (
                          <span className="launcher-enter-hint">
                            <span>{item.actionLabel || "Abrir"}</span>
                            <CornerDownLeft size={12} aria-hidden="true" />
                          </span>
                        )}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        )}

        {/* Dica interativa de calculadora na tela inicial */}
        {isEmptyQuery && (
          <div className="launcher-calc-tip" aria-label="Dica de calculadora instantânea">
            <div className="launcher-calc-tip-head">
              <Calculator size={14} aria-hidden="true" />
              <span>Cálculo instantâneo direto na busca</span>
            </div>
            <div className="launcher-calc-examples">
              {CALC_EXAMPLES.map((expr) => (
                <button
                  key={expr}
                  type="button"
                  className="launcher-example-pill"
                  onClick={() => {
                    setCategory("all");
                    setQuery(expr);
                    inputRef.current?.focus();
                  }}
                >
                  {expr}
                </button>
              ))}
            </div>
          </div>
        )}

        {/* Estado vazio quando nada bateu com a consulta */}
        {!isEmptyQuery && !response.calc && response.items.length === 0 && (
          <div className="launcher-empty">
            <Search size={24} strokeWidth={1.6} aria-hidden="true" />
            <strong>Nenhum resultado para “{query.trim()}”</strong>
            <p>
              Tente outro termo, use <code>f:</code> para buscar somente arquivos locais ou digite
              uma expressão como <code>1280 * 0.85</code>.
            </p>
          </div>
        )}

        {/* Dica na aba Arquivos quando o campo está vazio */}
        {isEmptyQuery && category === "files" && response.recent.length === 0 && (
          <div className="launcher-empty">
            <Folder size={24} strokeWidth={1.6} aria-hidden="true" />
            <strong>Busca rápida em pastas locais</strong>
            <p>
              Digite ao menos 2 caracteres para localizar arquivos e pastas em Documentos,
              Downloads, Área de Trabalho e pastas personalizadas.
            </p>
          </div>
        )}
      </section>

      <footer className="launcher-footer">
        <div className="launcher-footer-hints" aria-hidden="true">
          <span>
            <kbd className="launcher-kbd">↑</kbd>
            <kbd className="launcher-kbd">↓</kbd> Navegar
          </span>
          <span>
            <kbd className="launcher-kbd">↵</kbd> Executar
          </span>
          <span>
            <kbd className="launcher-kbd">Tab</kbd> Filtrar
          </span>
        </div>

        <div className="launcher-footer-status">
          <span className="launcher-index-meta">
            {response.isIndexing || refreshingIndex
              ? "Indexando aplicativos…"
              : `${response.indexedAppCount} apps no índice`}
          </span>
          <button
            type="button"
            className={`launcher-refresh-btn${refreshingIndex ? " is-spinning" : ""}`}
            onClick={() => void handleRefreshIndex()}
            disabled={refreshingIndex}
            aria-label="Atualizar índice de aplicativos"
            title="Atualizar índice de aplicativos"
          >
            <RefreshCw size={13} strokeWidth={2.1} />
          </button>
        </div>
      </footer>
    </main>
  );
}

export default LauncherWindow;
