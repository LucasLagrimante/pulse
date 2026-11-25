import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ArrowUpDown,
  ChevronRight,
  Code2,
  Copy,
  Download,
  ExternalLink,
  Eye,
  EyeOff,
  File,
  FileImage,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  HardDrive,
  Home,
  Image as ImageIcon,
  LayoutGrid,
  List,
  Monitor,
  Music,
  PanelRightClose,
  PanelRightOpen,
  Pencil,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  Search,
  Terminal,
  Trash2,
  Video,
  X,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import "./FileExplorer.css";

export type ExplorerDrive = {
  path: string;
  label: string;
  kind: string;
  totalBytes: number | null;
  freeBytes: number | null;
};

export type QuickAccessFolder = {
  id: string;
  name: string;
  path: string;
};

export type PinnedFolder = {
  name: string;
  path: string;
  pinnedAtUnixMs: number;
};

export type QuickAccessData = {
  drives: ExplorerDrive[];
  userFolders: QuickAccessFolder[];
  pins: PinnedFolder[];
  defaultPath: string;
};

export type ExplorerEntry = {
  name: string;
  path: string;
  isDir: boolean;
  isSymlink: boolean;
  isHidden: boolean;
  sizeBytes: number | null;
  modifiedUnixMs: number | null;
  extension: string | null;
  readonly: boolean;
};

export type BreadcrumbSegment = {
  name: string;
  path: string;
};

export type DirectoryListing = {
  path: string;
  parentPath: string | null;
  breadcrumbs: BreadcrumbSegment[];
  entries: ExplorerEntry[];
  truncated: boolean;
  totalSeen: number;
  dirCount: number;
  fileCount: number;
};

export type FilePreview = {
  path: string;
  name: string;
  isDir: boolean;
  isSymlink: boolean;
  isHidden: boolean;
  readonly: boolean;
  sizeBytes: number | null;
  modifiedUnixMs: number | null;
  createdUnixMs: number | null;
  extension: string | null;
  kind: "directory" | "text" | "image" | "binary" | string;
  textSnippet: string | null;
  textTruncated: boolean;
  imageDataUrl: string | null;
  note: string | null;
};

export type SortColumn = "name" | "modified" | "type" | "size";
export type SortDirection = "asc" | "desc";
export type ViewMode = "list" | "grid";

type ExplorerTab = {
  id: string;
  title: string;
  currentPath: string;
  history: string[];
  historyIndex: number;
  listing: DirectoryListing | null;
  loading: boolean;
  error: string | null;
  selectedPath: string | null;
  searchResults: ExplorerEntry[] | null;
  searchLoading: boolean;
  searchQueryApplied: string | null;
};

type DialogState =
  | { kind: "createFolder"; name: string }
  | { kind: "rename"; entry: ExplorerEntry; name: string }
  | { kind: "trash"; entry: ExplorerEntry }
  | null;

type ContextMenuState = {
  x: number;
  y: number;
  entry: ExplorerEntry;
};

type StatusBanner = {
  tone: "info" | "error";
  text: string;
};

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "webp", "gif", "svg", "bmp", "ico"]);
const CODE_EXTS = new Set([
  "ts",
  "tsx",
  "js",
  "jsx",
  "rs",
  "py",
  "go",
  "java",
  "c",
  "cpp",
  "h",
  "json",
  "toml",
  "yaml",
  "yml",
  "css",
  "html",
  "sql",
  "sh",
  "ps1",
]);
const TEXT_EXTS = new Set(["txt", "md", "log", "csv", "ini", "cfg", "env", "xml"]);

export function formatExplorerSize(bytes: number | null | undefined): string {
  if (bytes == null || typeof bytes !== "number" || Number.isNaN(bytes) || bytes < 0) {
    return "Indisponível";
  }
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1).replace(".", ",")} KB`;
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1).replace(".", ",")} MB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1).replace(".", ",")} GB`;
}

export function formatExplorerDate(timestampMs: number | null | undefined): string {
  if (
    timestampMs == null ||
    typeof timestampMs !== "number" ||
    Number.isNaN(timestampMs) ||
    timestampMs <= 0
  ) {
    return "Data indisponível";
  }
  const date = new Date(timestampMs);
  if (Number.isNaN(date.getTime())) return "Data indisponível";
  return date.toLocaleString("pt-BR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function entryTypeLabel(entry: ExplorerEntry): string {
  if (entry.isDir) return "Pasta";
  if (entry.extension) return entry.extension.toUpperCase();
  return "Arquivo";
}

function normalizeComparePath(raw: string): { key: string; caseInsensitive: boolean } {
  const trimmed = raw.trim();
  const isWin = /^[A-Za-z]:/.test(trimmed) || trimmed.startsWith("\\\\");
  if (isWin) {
    const slashNorm = trimmed.replace(/\//g, "\\");
    const clean =
      slashNorm.length === 2 && slashNorm[1] === ":"
        ? `${slashNorm}\\`
        : slashNorm.length > 3
          ? slashNorm.replace(/\\+$/, "")
          : slashNorm;
    return { key: clean.toLowerCase(), caseInsensitive: true };
  }
  const clean = trimmed === "/" ? "/" : trimmed.replace(/\/+$/, "");
  return { key: clean, caseInsensitive: false };
}

export function pathsEqual(a: string | null | undefined, b: string | null | undefined): boolean {
  if (!a || !b) return false;
  const na = normalizeComparePath(a);
  const nb = normalizeComparePath(b);
  return na.key === nb.key;
}

export function isPathPinned(pins: PinnedFolder[], path: string): boolean {
  return pins.some(pin => pathsEqual(pin.path, path));
}

export function folderTitleFromPath(path: string): string {
  const trimmed = path.trim();
  if (!trimmed) return "Explorador";
  if (trimmed === "/") return "/";
  if (/^[A-Za-z]:\\?$/.test(trimmed)) return trimmed.slice(0, 2);
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? trimmed;
}

export function sortExplorerEntries(
  entries: ExplorerEntry[],
  sortCol: SortColumn,
  sortDir: SortDirection,
): ExplorerEntry[] {
  const factor = sortDir === "asc" ? 1 : -1;
  return [...entries].sort((a, b) => {
    // Mantém pastas sempre agrupadas antes dos arquivos
    if (a.isDir !== b.isDir) {
      return a.isDir ? -1 : 1;
    }
    let cmp = 0;
    if (sortCol === "name") {
      cmp = a.name.localeCompare(b.name, "pt-BR", { sensitivity: "base" });
    } else if (sortCol === "modified") {
      cmp = (a.modifiedUnixMs ?? 0) - (b.modifiedUnixMs ?? 0);
    } else if (sortCol === "type") {
      cmp = entryTypeLabel(a).localeCompare(entryTypeLabel(b), "pt-BR", {
        sensitivity: "base",
      });
    } else if (sortCol === "size") {
      cmp = (a.sizeBytes ?? -1) - (b.sizeBytes ?? -1);
    }
    if (cmp !== 0) return cmp * factor;
    return a.name.localeCompare(b.name, "pt-BR", { sensitivity: "base" });
  });
}

function QuickFolderIcon({ id }: { id: string }) {
  switch (id) {
    case "home":
      return <Home size={15} aria-hidden="true" />;
    case "desktop":
      return <Monitor size={15} aria-hidden="true" />;
    case "downloads":
      return <Download size={15} aria-hidden="true" />;
    case "documents":
      return <FileText size={15} aria-hidden="true" />;
    case "pictures":
      return <ImageIcon size={15} aria-hidden="true" />;
    case "videos":
      return <Video size={15} aria-hidden="true" />;
    case "music":
      return <Music size={15} aria-hidden="true" />;
    default:
      return <Folder size={15} aria-hidden="true" />;
  }
}

function EntryGlyph({ entry, size = 17 }: { entry: ExplorerEntry; size?: number }) {
  if (entry.isDir) {
    return <Folder size={size} className="fe-glyph-dir" aria-hidden="true" />;
  }
  const ext = entry.extension?.toLowerCase() ?? "";
  if (IMAGE_EXTS.has(ext)) {
    return <FileImage size={size} className="fe-glyph-image" aria-hidden="true" />;
  }
  if (CODE_EXTS.has(ext)) {
    return <Code2 size={size} className="fe-glyph-code" aria-hidden="true" />;
  }
  if (TEXT_EXTS.has(ext)) {
    return <FileText size={size} className="fe-glyph-file" aria-hidden="true" />;
  }
  return <File size={size} className="fe-glyph-file" aria-hidden="true" />;
}

function formatErrorMessage(err: unknown, fallback: string): string {
  if (typeof err === "string" && err.trim() !== "") return err;
  if (err instanceof Error && err.message.trim() !== "") return err.message;
  return fallback;
}

export function FileExplorer() {
  const tabCounterRef = useRef(2);
  const tabRequestSeqRef = useRef<Record<string, number>>({});
  const previewSeqRef = useRef(0);
  const pathInputRef = useRef<HTMLInputElement | null>(null);
  const modalInputRef = useRef<HTMLInputElement | null>(null);

  const [quickAccess, setQuickAccess] = useState<QuickAccessData | null>(null);
  const [tabs, setTabs] = useState<ExplorerTab[]>([
    {
      id: "tab-1",
      title: "Início",
      currentPath: "",
      history: [],
      historyIndex: -1,
      listing: null,
      loading: true,
      error: null,
      selectedPath: null,
      searchResults: null,
      searchLoading: false,
      searchQueryApplied: null,
    },
  ]);
  const [activeTabId, setActiveTabId] = useState<string>("tab-1");
  const [viewMode, setViewMode] = useState<ViewMode>("list");
  const [showHidden, setShowHidden] = useState(false);
  const [sortCol, setSortCol] = useState<SortColumn>("name");
  const [sortDir, setSortDir] = useState<SortDirection>("asc");
  const [filterQuery, setFilterQuery] = useState("");
  const [editingPath, setEditingPath] = useState(false);
  const [pathDraft, setPathDraft] = useState("");
  const [previewOpen, setPreviewOpen] = useState(true);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogState>(null);
  const [dialogBusy, setDialogBusy] = useState(false);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [banner, setBanner] = useState<StatusBanner | null>(null);

  const activeTab = useMemo(
    () => tabs.find(t => t.id === activeTabId) ?? tabs[0],
    [tabs, activeTabId],
  );

  const updateTab = useCallback((tabId: string, updater: (prev: ExplorerTab) => ExplorerTab) => {
    setTabs(prev => prev.map(tab => (tab.id === tabId ? updater(tab) : tab)));
  }, []);

  const loadDirectoryIntoTab = useCallback(
    async (
      tabId: string,
      targetPath: string,
      options: { pushHistory: boolean; hiddenOverride?: boolean; selectPath?: string | null },
    ) => {
      const nextSeq = (tabRequestSeqRef.current[tabId] ?? 0) + 1;
      tabRequestSeqRef.current[tabId] = nextSeq;

      const hiddenFlag = options.hiddenOverride ?? showHidden;

      updateTab(tabId, prev => ({
        ...prev,
        loading: true,
        error: null,
        searchResults: null,
        searchQueryApplied: null,
      }));

      try {
        const listing = await invoke<DirectoryListing>("explorer_list_directory", {
          path: targetPath,
          showHidden: hiddenFlag,
        });
        if (tabRequestSeqRef.current[tabId] !== nextSeq) return;

        updateTab(tabId, prev => {
          let nextHistory = prev.history;
          let nextIndex = prev.historyIndex;

          if (options.pushHistory) {
            const head = prev.history.slice(0, prev.historyIndex + 1);
            if (head.length === 0 || !pathsEqual(head[head.length - 1], listing.path)) {
              nextHistory = [...head, listing.path];
              nextIndex = nextHistory.length - 1;
            } else {
              nextHistory = head;
              nextIndex = head.length - 1;
            }
          } else if (prev.history.length === 0) {
            nextHistory = [listing.path];
            nextIndex = 0;
          }

          return {
            ...prev,
            title: folderTitleFromPath(listing.path),
            currentPath: listing.path,
            history: nextHistory,
            historyIndex: nextIndex,
            listing,
            loading: false,
            error: null,
            selectedPath:
              options.selectPath !== undefined
                ? options.selectPath
                : prev.currentPath === listing.path
                  ? prev.selectedPath
                  : null,
          };
        });
      } catch (err) {
        if (tabRequestSeqRef.current[tabId] !== nextSeq) return;
        const message = formatErrorMessage(err, "Não foi possível abrir esta pasta.");
        updateTab(tabId, prev => ({
          ...prev,
          loading: false,
          error: message,
        }));
      }
    },
    [showHidden, updateTab],
  );

  // Carregamento inicial de Acesso Rápido + primeira pasta
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      let initialPath = "";
      try {
        const data = await invoke<QuickAccessData>("explorer_get_quick_access");
        if (cancelled) return;
        setQuickAccess(data);
        initialPath = data.defaultPath ?? "";
      } catch (err) {
        if (!cancelled) {
          setBanner({
            tone: "error",
            text: formatErrorMessage(err, "Não foi possível consultar unidades e pastas rápidas."),
          });
        }
      }
      if (!cancelled) {
        await loadDirectoryIntoTab("tab-1", initialPath, { pushHistory: true });
      }
    })();
    return () => {
      cancelled = true;
    };
    // Executado apenas na montagem inicial
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Foca e seleciona input de caminho ao entrar em modo de edição
  useEffect(() => {
    if (editingPath) {
      pathInputRef.current?.focus();
      pathInputRef.current?.select();
    }
  }, [editingPath]);

  // Foca input de modal (Nova pasta / Renomear)
  useEffect(() => {
    if (dialog && (dialog.kind === "createFolder" || dialog.kind === "rename")) {
      modalInputRef.current?.focus();
      modalInputRef.current?.select();
    }
  }, [dialog]);

  // Fecha menu de contexto ao clicar fora ou pressionar Escape
  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    const onKeyDown = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [contextMenu]);

  // Atualiza pré-visualização quando item selecionado ou estado do painel muda
  const selectedEntry = useMemo(() => {
    if (!activeTab?.selectedPath) return null;
    const pool = activeTab.searchResults ?? activeTab.listing?.entries ?? [];
    return pool.find(e => e.path === activeTab.selectedPath) ?? null;
  }, [activeTab]);

  useEffect(() => {
    if (!previewOpen || !selectedEntry) {
      setPreview(null);
      setPreviewError(null);
      setPreviewLoading(false);
      return;
    }

    const seq = ++previewSeqRef.current;
    setPreview(null);
    setPreviewLoading(true);
    setPreviewError(null);

    void invoke<FilePreview>("explorer_preview_file", { path: selectedEntry.path })
      .then(result => {
        if (previewSeqRef.current !== seq) return;
        setPreview(result);
        setPreviewLoading(false);
      })
      .catch(err => {
        if (previewSeqRef.current !== seq) return;
        setPreview(null);
        setPreviewError(formatErrorMessage(err, "Pré-visualização indisponível."));
        setPreviewLoading(false);
      });
  }, [previewOpen, selectedEntry]);

  const navigateTo = useCallback(
    (targetPath: string) => {
      setFilterQuery("");
      setEditingPath(false);
      setContextMenu(null);
      void loadDirectoryIntoTab(activeTab.id, targetPath, {
        pushHistory: true,
        selectPath: null,
      });
    },
    [activeTab.id, loadDirectoryIntoTab],
  );

  const handleBack = () => {
    if (activeTab.historyIndex <= 0) return;
    const nextIndex = activeTab.historyIndex - 1;
    const target = activeTab.history[nextIndex];
    updateTab(activeTab.id, prev => ({ ...prev, historyIndex: nextIndex }));
    setFilterQuery("");
    setEditingPath(false);
    void loadDirectoryIntoTab(activeTab.id, target, { pushHistory: false, selectPath: null });
  };

  const handleForward = () => {
    if (activeTab.historyIndex >= activeTab.history.length - 1) return;
    const nextIndex = activeTab.historyIndex + 1;
    const target = activeTab.history[nextIndex];
    updateTab(activeTab.id, prev => ({ ...prev, historyIndex: nextIndex }));
    setFilterQuery("");
    setEditingPath(false);
    void loadDirectoryIntoTab(activeTab.id, target, { pushHistory: false, selectPath: null });
  };

  const handleUp = () => {
    const parent = activeTab.listing?.parentPath;
    if (!parent) return;
    navigateTo(parent);
  };

  const handleReload = useCallback(() => {
    if (!activeTab.currentPath) return;
    void loadDirectoryIntoTab(activeTab.id, activeTab.currentPath, {
      pushHistory: false,
      selectPath: activeTab.selectedPath,
    });
  }, [activeTab.currentPath, activeTab.id, activeTab.selectedPath, loadDirectoryIntoTab]);

  const handleToggleHidden = () => {
    const next = !showHidden;
    setShowHidden(next);
    if (activeTab.currentPath) {
      void loadDirectoryIntoTab(activeTab.id, activeTab.currentPath, {
        pushHistory: false,
        hiddenOverride: next,
        selectPath: activeTab.selectedPath,
      });
    }
  };

  const handleNewTab = () => {
    const id = `tab-${tabCounterRef.current++}`;
    const startPath = activeTab.currentPath || quickAccess?.defaultPath || "";
    const nextTab: ExplorerTab = {
      id,
      title: folderTitleFromPath(startPath),
      currentPath: startPath,
      history: startPath ? [startPath] : [],
      historyIndex: startPath ? 0 : -1,
      listing: null,
      loading: true,
      error: null,
      selectedPath: null,
      searchResults: null,
      searchLoading: false,
      searchQueryApplied: null,
    };
    setTabs(prev => [...prev, nextTab]);
    setActiveTabId(id);
    setFilterQuery("");
    setEditingPath(false);
    void loadDirectoryIntoTab(id, startPath, { pushHistory: true });
  };

  const handleCloseTab = (tabId: string) => {
    if (tabs.length <= 1) return;
    const idx = tabs.findIndex(t => t.id === tabId);
    const nextTabs = tabs.filter(t => t.id !== tabId);
    setTabs(nextTabs);
    if (activeTabId === tabId) {
      const fallback = nextTabs[Math.max(0, idx - 1)] ?? nextTabs[0];
      setActiveTabId(fallback.id);
    }
  };

  const handleDeepSearch = async () => {
    const q = filterQuery.trim();
    if (!q || !activeTab.currentPath) return;
    const tabId = activeTab.id;
    updateTab(tabId, prev => ({ ...prev, searchLoading: true, error: null }));
    try {
      const results = await invoke<ExplorerEntry[]>("explorer_search_in_dir", {
        root: activeTab.currentPath,
        query: q,
        maxResults: 200,
      });
      updateTab(tabId, prev => ({
        ...prev,
        searchResults: results,
        searchLoading: false,
        searchQueryApplied: q,
        selectedPath: null,
      }));
    } catch (err) {
      updateTab(tabId, prev => ({
        ...prev,
        searchLoading: false,
      }));
      setBanner({
        tone: "error",
        text: formatErrorMessage(err, "Falha ao pesquisar nas subpastas."),
      });
    }
  };

  const handleClearSearch = () => {
    setFilterQuery("");
    updateTab(activeTab.id, prev => ({
      ...prev,
      searchResults: null,
      searchQueryApplied: null,
      searchLoading: false,
    }));
  };

  const handleOpenEntry = useCallback(
    async (entry: ExplorerEntry, forceSystemOpen = false) => {
      if (entry.isDir && !forceSystemOpen) {
        navigateTo(entry.path);
        return;
      }
      try {
        await invoke("explorer_open_entry", { path: entry.path });
      } catch (err) {
        setBanner({
          tone: "error",
          text: formatErrorMessage(err, `Não foi possível abrir '${entry.name}'.`),
        });
      }
    },
    [navigateTo],
  );

  const handleOpenInTerminal = async (path: string) => {
    try {
      await invoke("explorer_open_in_terminal", { path });
    } catch (err) {
      setBanner({
        tone: "error",
        text: formatErrorMessage(err, "Não foi possível abrir o terminal nesta pasta."),
      });
    }
  };

  const handleCopyPath = async (path: string) => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(path);
      }
      setBanner({ tone: "info", text: `Caminho copiado: ${path}` });
    } catch {
      setBanner({ tone: "error", text: "Não foi possível copiar para a área de transferência." });
    }
  };

  const handleTogglePin = async (path: string) => {
    try {
      const updatedPins = await invoke<PinnedFolder[]>("explorer_toggle_pin", { path });
      setQuickAccess(prev => (prev ? { ...prev, pins: updatedPins } : prev));
    } catch (err) {
      setBanner({
        tone: "error",
        text: formatErrorMessage(err, "Não foi possível atualizar os favoritos."),
      });
    }
  };

  const submitDialog = async () => {
    if (!dialog || dialogBusy) return;
    setDialogBusy(true);
    setDialogError(null);
    try {
      if (dialog.kind === "createFolder") {
        const created = await invoke<ExplorerEntry>("explorer_create_folder", {
          parentPath: activeTab.currentPath,
          name: dialog.name,
        });
        setDialog(null);
        setBanner({ tone: "info", text: `Pasta '${created.name}' criada.` });
        await loadDirectoryIntoTab(activeTab.id, activeTab.currentPath, {
          pushHistory: false,
          selectPath: created.path,
        });
      } else if (dialog.kind === "rename") {
        const renamed = await invoke<ExplorerEntry>("explorer_rename_entry", {
          path: dialog.entry.path,
          newName: dialog.name,
        });
        setDialog(null);
        setBanner({ tone: "info", text: `Renomeado para '${renamed.name}'.` });
        await loadDirectoryIntoTab(activeTab.id, activeTab.currentPath, {
          pushHistory: false,
          selectPath: renamed.path,
        });
      } else if (dialog.kind === "trash") {
        await invoke("explorer_trash_entry", { path: dialog.entry.path });
        setDialog(null);
        setBanner({
          tone: "info",
          text: `'${dialog.entry.name}' foi movido para a Lixeira do sistema.`,
        });
        await loadDirectoryIntoTab(activeTab.id, activeTab.currentPath, {
          pushHistory: false,
          selectPath: null,
        });
      }
    } catch (err) {
      setDialogError(formatErrorMessage(err, "Não foi possível concluir a operação."));
    } finally {
      setDialogBusy(false);
    }
  };

  const handleSortClick = (column: SortColumn) => {
    if (sortCol === column) {
      setSortDir(prev => (prev === "asc" ? "desc" : "asc"));
    } else {
      setSortCol(column);
      setSortDir(column === "modified" || column === "size" ? "desc" : "asc");
    }
  };

  const visibleEntries = useMemo(() => {
    const base = activeTab.searchResults ?? activeTab.listing?.entries ?? [];
    const needle = filterQuery.trim().toLowerCase();
    const filtered =
      activeTab.searchResults != null || needle === ""
        ? base
        : base.filter(e => e.name.toLowerCase().includes(needle));
    return sortExplorerEntries(filtered, sortCol, sortDir);
  }, [activeTab.listing?.entries, activeTab.searchResults, filterQuery, sortCol, sortDir]);

  const beginPathEdit = () => {
    setPathDraft(activeTab.currentPath);
    setEditingPath(true);
  };

  const commitPathEdit = () => {
    const next = pathDraft.trim();
    setEditingPath(false);
    if (next && !pathsEqual(next, activeTab.currentPath)) {
      navigateTo(next);
    }
  };

  const handlePathKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      commitPathEdit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      setEditingPath(false);
    }
  };

  const handleEntryContextMenu = (e: ReactMouseEvent, entry: ExplorerEntry) => {
    e.preventDefault();
    e.stopPropagation();
    updateTab(activeTab.id, prev => ({ ...prev, selectedPath: entry.path }));
    const maxX = typeof window !== "undefined" ? Math.max(12, window.innerWidth - 230) : e.clientX;
    const maxY = typeof window !== "undefined" ? Math.max(12, window.innerHeight - 260) : e.clientY;
    setContextMenu({
      x: Math.min(e.clientX, maxX),
      y: Math.min(e.clientY, maxY),
      entry,
    });
  };

  const handleEntryKeyDown = (e: ReactKeyboardEvent, entry: ExplorerEntry) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void handleOpenEntry(entry);
    } else if (e.key === "F2") {
      e.preventDefault();
      setDialogError(null);
      setDialog({ kind: "rename", entry, name: entry.name });
    } else if (e.key === "Delete") {
      e.preventDefault();
      setDialogError(null);
      setDialog({ kind: "trash", entry });
    }
  };

  const currentFolderPinned = useMemo(
    () => (quickAccess ? isPathPinned(quickAccess.pins, activeTab.currentPath) : false),
    [quickAccess, activeTab.currentPath],
  );

  const selectedEntryPinned = useMemo(
    () =>
      quickAccess && selectedEntry?.isDir
        ? isPathPinned(quickAccess.pins, selectedEntry.path)
        : false,
    [quickAccess, selectedEntry],
  );

  return (
    <section className="fe-root" aria-label="Explorador de Arquivos">
      {/* Barra de Abas */}
      <div className="fe-tabs-bar" role="tablist" aria-label="Abas do explorador">
        {tabs.map(tab => {
          const isActive = tab.id === activeTab.id;
          return (
            <div key={tab.id} className={`fe-tab-group${isActive ? " is-active" : ""}`}>
              <button
                type="button"
                role="tab"
                aria-selected={isActive}
                className="fe-tab-btn"
                title={tab.currentPath || tab.title}
                onClick={() => {
                  setActiveTabId(tab.id);
                  setEditingPath(false);
                  setContextMenu(null);
                }}
              >
                <Folder size={13} aria-hidden="true" />
                <span className="fe-tab-title">{tab.title}</span>
              </button>
              {tabs.length > 1 && (
                <button
                  type="button"
                  className="fe-tab-close"
                  aria-label={`Fechar aba ${tab.title}`}
                  onClick={() => handleCloseTab(tab.id)}
                >
                  <X size={12} aria-hidden="true" />
                </button>
              )}
            </div>
          );
        })}
        <button
          type="button"
          className="fe-tab-add"
          aria-label="Nova aba"
          title="Abrir nova aba"
          onClick={handleNewTab}
        >
          <Plus size={14} aria-hidden="true" />
        </button>
      </div>

      {/* Barra de Navegação e Ferramentas */}
      <header className="fe-toolbar">
        <div className="fe-nav-buttons">
          <button
            type="button"
            className="fe-icon-btn"
            aria-label="Voltar"
            title="Voltar"
            disabled={activeTab.historyIndex <= 0}
            onClick={handleBack}
          >
            <ArrowLeft size={15} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="fe-icon-btn"
            aria-label="Avançar"
            title="Avançar"
            disabled={activeTab.historyIndex >= activeTab.history.length - 1}
            onClick={handleForward}
          >
            <ArrowRight size={15} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="fe-icon-btn"
            aria-label="Pasta acima"
            title="Pasta acima"
            disabled={!activeTab.listing?.parentPath}
            onClick={handleUp}
          >
            <ArrowUp size={15} aria-hidden="true" />
          </button>
          <button
            type="button"
            className="fe-icon-btn"
            aria-label="Recarregar"
            title="Recarregar pasta"
            disabled={!activeTab.currentPath || activeTab.loading}
            onClick={handleReload}
          >
            <RefreshCw size={14} aria-hidden="true" />
          </button>
        </div>

        {/* Breadcrumbs clicáveis / Campo de caminho editável */}
        <div
          className="fe-path-bar"
          onClick={() => {
            if (!editingPath) beginPathEdit();
          }}
        >
          {editingPath ? (
            <input
              ref={pathInputRef}
              type="text"
              className="fe-path-input"
              aria-label="Caminho da pasta"
              value={pathDraft}
              onChange={e => setPathDraft(e.target.value)}
              onKeyDown={handlePathKeyDown}
              onBlur={() => setEditingPath(false)}
            />
          ) : (
            <>
              <nav className="fe-breadcrumbs" aria-label="Caminho atual">
                {(activeTab.listing?.breadcrumbs ?? []).map((crumb, idx, arr) => {
                  const isLast = idx === arr.length - 1;
                  return (
                    <span key={crumb.path} className="fe-crumb-wrap">
                      {idx > 0 && <ChevronRight size={13} className="fe-crumb-sep" aria-hidden="true" />}
                      <button
                        type="button"
                        className={`fe-crumb-btn${isLast ? " is-last" : ""}`}
                        onClick={e => {
                          e.stopPropagation();
                          navigateTo(crumb.path);
                        }}
                      >
                        {crumb.name}
                      </button>
                    </span>
                  );
                })}
              </nav>
              <span className="fe-path-spacer" aria-hidden="true" />
              <button
                type="button"
                className="fe-path-edit-btn"
                aria-label="Editar caminho"
                title="Digitar caminho diretamente"
                onClick={e => {
                  e.stopPropagation();
                  beginPathEdit();
                }}
              >
                <Pencil size={12} aria-hidden="true" />
              </button>
            </>
          )}
        </div>

        {/* Busca instantânea + Pesquisa recursiva */}
        <div className="fe-search-group">
          <div className="fe-search-box">
            <Search size={14} aria-hidden="true" />
            <input
              type="search"
              className="fe-search-input"
              aria-label="Filtrar itens ou pesquisar"
              placeholder="Filtrar pasta…"
              value={filterQuery}
              onChange={e => {
                const value = e.target.value;
                setFilterQuery(value);
                if (value.trim() === "" && activeTab.searchResults !== null) {
                  updateTab(activeTab.id, prev => ({
                    ...prev,
                    searchResults: null,
                    searchQueryApplied: null,
                  }));
                }
              }}
              onKeyDown={e => {
                if (e.key === "Enter" && filterQuery.trim() !== "") {
                  e.preventDefault();
                  void handleDeepSearch();
                } else if (e.key === "Escape") {
                  handleClearSearch();
                }
              }}
            />
            {(filterQuery !== "" || activeTab.searchResults !== null) && (
              <button
                type="button"
                className="fe-path-edit-btn"
                aria-label="Limpar busca"
                onClick={handleClearSearch}
              >
                <X size={12} aria-hidden="true" />
              </button>
            )}
          </div>
          <button
            type="button"
            className="fe-pill-btn"
            disabled={!filterQuery.trim() || !activeTab.currentPath || activeTab.searchLoading}
            onClick={() => void handleDeepSearch()}
          >
            <Search size={13} aria-hidden="true" />
            <span>{activeTab.searchLoading ? "Buscando…" : "Buscar nas subpastas"}</span>
          </button>
        </div>

        {/* Alternadores de visualização e ações da pasta */}
        <div className="fe-toolbar-actions">
          <div className="fe-segmented" role="group" aria-label="Modo de visualização">
            <button
              type="button"
              className={`fe-seg-btn${viewMode === "list" ? " is-active" : ""}`}
              aria-label="Visualização em lista"
              aria-pressed={viewMode === "list"}
              title="Lista"
              onClick={() => setViewMode("list")}
            >
              <List size={14} aria-hidden="true" />
            </button>
            <button
              type="button"
              className={`fe-seg-btn${viewMode === "grid" ? " is-active" : ""}`}
              aria-label="Visualização em grade"
              aria-pressed={viewMode === "grid"}
              title="Grade"
              onClick={() => setViewMode("grid")}
            >
              <LayoutGrid size={14} aria-hidden="true" />
            </button>
          </div>

          <button
            type="button"
            className={`fe-pill-btn${showHidden ? " is-active" : ""}`}
            aria-pressed={showHidden}
            onClick={handleToggleHidden}
          >
            {showHidden ? <Eye size={13} aria-hidden="true" /> : <EyeOff size={13} aria-hidden="true" />}
            <span>Ocultos</span>
          </button>

          <button
            type="button"
            className="fe-pill-btn"
            disabled={!activeTab.currentPath || activeTab.loading}
            onClick={() => {
              setDialogError(null);
              setDialog({ kind: "createFolder", name: "" });
            }}
          >
            <FolderPlus size={13} aria-hidden="true" />
            <span>Nova pasta</span>
          </button>

          <button
            type="button"
            className={`fe-icon-btn${previewOpen ? " is-active" : ""}`}
            aria-label={previewOpen ? "Ocultar pré-visualização" : "Mostrar pré-visualização"}
            aria-pressed={previewOpen}
            title="Painel de pré-visualização"
            onClick={() => setPreviewOpen(prev => !prev)}
          >
            {previewOpen ? <PanelRightClose size={15} aria-hidden="true" /> : <PanelRightOpen size={15} aria-hidden="true" />}
          </button>
        </div>
      </header>

      {/* Área Principal: Sidebar + Lista/Grade + Preview */}
      <div className="fe-body">
        <aside className="fe-sidebar" aria-label="Acesso rápido e unidades">
          <div className="fe-side-section">
            <h3 className="fe-side-title">Acesso Rápido</h3>
            {(quickAccess?.userFolders ?? []).map(folder => {
              const active = pathsEqual(activeTab.currentPath, folder.path);
              return (
                <button
                  key={folder.id}
                  type="button"
                  className={`fe-side-item${active ? " is-active" : ""}`}
                  title={folder.path}
                  onClick={() => navigateTo(folder.path)}
                >
                  <QuickFolderIcon id={folder.id} />
                  <span className="fe-side-item-label">{folder.name}</span>
                </button>
              );
            })}
          </div>

          <div className="fe-side-section">
            <h3 className="fe-side-title">Fixados</h3>
            {!quickAccess || quickAccess.pins.length === 0 ? (
              <p className="fe-side-empty">Nenhuma pasta fixada ainda.</p>
            ) : (
              quickAccess.pins.map(pin => {
                const active = pathsEqual(activeTab.currentPath, pin.path);
                return (
                  <div key={pin.path} className="fe-pin-row">
                    <button
                      type="button"
                      className={`fe-side-item${active ? " is-active" : ""}`}
                      title={pin.path}
                      onClick={() => navigateTo(pin.path)}
                    >
                      <Pin size={14} aria-hidden="true" />
                      <span className="fe-side-item-label">{pin.name}</span>
                    </button>
                    <button
                      type="button"
                      className="fe-pin-remove"
                      aria-label={`Desafixar ${pin.name}`}
                      title={`Desafixar ${pin.name}`}
                      onClick={() => void handleTogglePin(pin.path)}
                    >
                      <PinOff size={13} aria-hidden="true" />
                    </button>
                  </div>
                );
              })
            )}
          </div>

          <div className="fe-side-section">
            <h3 className="fe-side-title">Unidades</h3>
            {(quickAccess?.drives ?? []).map(drive => {
              const active = pathsEqual(activeTab.currentPath, drive.path);
              const hasCapacity =
                drive.totalBytes != null &&
                drive.freeBytes != null &&
                drive.totalBytes > 0 &&
                drive.freeBytes <= drive.totalBytes;
              const usedPercent = hasCapacity
                ? Math.round(((drive.totalBytes! - drive.freeBytes!) / drive.totalBytes!) * 100)
                : null;
              return (
                <button
                  key={drive.path}
                  type="button"
                  className={`fe-drive-card${active ? " is-active" : ""}`}
                  title={drive.path}
                  onClick={() => navigateTo(drive.path)}
                >
                  <div className="fe-drive-head">
                    <HardDrive size={14} aria-hidden="true" />
                    <span>{drive.label}</span>
                  </div>
                  {usedPercent != null && (
                    <div className="fe-drive-bar" aria-hidden="true">
                      <div
                        className={`fe-drive-fill${usedPercent >= 90 ? " is-high" : ""}`}
                        style={{ width: `${Math.min(100, Math.max(2, usedPercent))}%` }}
                      />
                    </div>
                  )}
                  <span className="fe-drive-meta">
                    {hasCapacity
                      ? `${formatExplorerSize(drive.freeBytes)} livres de ${formatExplorerSize(drive.totalBytes)}`
                      : "Capacidade indisponível"}
                  </span>
                </button>
              );
            })}
          </div>
        </aside>

        <main className="fe-main">
          {/* Barra de ações contextuais */}
          <div className="fe-action-bar">
            {selectedEntry ? (
              <>
                <div className="fe-action-summary">
                  <EntryGlyph entry={selectedEntry} size={15} />
                  <strong>{selectedEntry.name}</strong>
                  <span>{entryTypeLabel(selectedEntry)}</span>
                </div>
                <div className="fe-action-buttons">
                  <button
                    type="button"
                    className="fe-pill-btn"
                    onClick={() => void handleOpenEntry(selectedEntry)}
                  >
                    <ExternalLink size={13} aria-hidden="true" />
                    <span>Abrir</span>
                  </button>
                  <button
                    type="button"
                    className="fe-pill-btn"
                    onClick={() => void handleOpenInTerminal(selectedEntry.path)}
                  >
                    <Terminal size={13} aria-hidden="true" />
                    <span>Terminal</span>
                  </button>
                  <button
                    type="button"
                    className="fe-pill-btn"
                    onClick={() => void handleCopyPath(selectedEntry.path)}
                  >
                    <Copy size={13} aria-hidden="true" />
                    <span>Copiar caminho</span>
                  </button>
                  {selectedEntry.isDir && (
                    <button
                      type="button"
                      className="fe-pill-btn"
                      onClick={() => void handleTogglePin(selectedEntry.path)}
                    >
                      {selectedEntryPinned ? (
                        <PinOff size={13} aria-hidden="true" />
                      ) : (
                        <Pin size={13} aria-hidden="true" />
                      )}
                      <span>{selectedEntryPinned ? "Desafixar" : "Fixar pasta"}</span>
                    </button>
                  )}
                  <button
                    type="button"
                    className="fe-pill-btn"
                    onClick={() => {
                      setDialogError(null);
                      setDialog({ kind: "rename", entry: selectedEntry, name: selectedEntry.name });
                    }}
                  >
                    <Pencil size={13} aria-hidden="true" />
                    <span>Renomear</span>
                  </button>
                  <button
                    type="button"
                    className="fe-pill-btn is-danger"
                    onClick={() => {
                      setDialogError(null);
                      setDialog({ kind: "trash", entry: selectedEntry });
                    }}
                  >
                    <Trash2 size={13} aria-hidden="true" />
                    <span>Lixeira</span>
                  </button>
                </div>
              </>
            ) : (
              <>
                <div className="fe-action-summary">
                  <FolderOpen size={15} aria-hidden="true" />
                  <span>
                    {activeTab.listing
                      ? `${activeTab.listing.dirCount} ${activeTab.listing.dirCount === 1 ? "pasta" : "pastas"} · ${activeTab.listing.fileCount} ${activeTab.listing.fileCount === 1 ? "arquivo" : "arquivos"}`
                      : "Carregando pasta…"}
                  </span>
                </div>
                <div className="fe-action-buttons">
                  {activeTab.currentPath && (
                    <>
                      <button
                        type="button"
                        className="fe-pill-btn"
                        onClick={() => void handleTogglePin(activeTab.currentPath)}
                      >
                        {currentFolderPinned ? (
                          <PinOff size={13} aria-hidden="true" />
                        ) : (
                          <Pin size={13} aria-hidden="true" />
                        )}
                        <span>{currentFolderPinned ? "Desafixar pasta atual" : "Fixar pasta atual"}</span>
                      </button>
                      <button
                        type="button"
                        className="fe-pill-btn"
                        onClick={() => void handleOpenInTerminal(activeTab.currentPath)}
                      >
                        <Terminal size={13} aria-hidden="true" />
                        <span>Terminal aqui</span>
                      </button>
                    </>
                  )}
                </div>
              </>
            )}
          </div>

          {/* Avisos contextuais */}
          {activeTab.searchResults !== null && (
            <div className="fe-banner is-info">
              <span>
                Busca em subpastas para <strong>"{activeTab.searchQueryApplied}"</strong>:{" "}
                {activeTab.searchResults.length} resultado(s).
              </span>
              <button type="button" className="fe-pill-btn" onClick={handleClearSearch}>
                Voltar à pasta
              </button>
            </div>
          )}

          {activeTab.listing?.truncated && activeTab.searchResults === null && (
            <div className="fe-banner is-warn" role="status">
              <span>
                Pasta muito extensa: exibindo os primeiros 5.000 itens ({activeTab.listing.totalSeen}{" "}
                encontrados) para manter a interface responsiva.
              </span>
            </div>
          )}

          {banner && (
            <div
              className={`fe-banner ${banner.tone === "error" ? "is-error" : "is-info"}`}
              role={banner.tone === "error" ? "alert" : "status"}
            >
              <span>{banner.text}</span>
              <button
                type="button"
                className="fe-path-edit-btn"
                aria-label="Fechar aviso"
                onClick={() => setBanner(null)}
              >
                <X size={13} aria-hidden="true" />
              </button>
            </div>
          )}

          {/* Lista ou Grade de Entradas */}
          <div className="fe-entries-scroll">
            {activeTab.loading ? (
              <div className="fe-empty-state" role="status">
                <span>Lendo diretório…</span>
              </div>
            ) : activeTab.error ? (
              <div className="fe-empty-state" role="alert">
                <AlertTriangle size={22} aria-hidden="true" />
                <span>{activeTab.error}</span>
                <div className="fe-action-buttons">
                  <button type="button" className="fe-pill-btn" onClick={handleReload}>
                    Tentar novamente
                  </button>
                  {quickAccess?.defaultPath && (
                    <button
                      type="button"
                      className="fe-pill-btn"
                      onClick={() => navigateTo(quickAccess.defaultPath)}
                    >
                      Voltar para Início
                    </button>
                  )}
                </div>
              </div>
            ) : visibleEntries.length === 0 ? (
              <div className="fe-empty-state">
                <Folder size={24} aria-hidden="true" />
                <span>
                  {filterQuery.trim() !== "" || activeTab.searchResults !== null
                    ? "Nenhum item corresponde ao filtro informado."
                    : "Esta pasta está vazia."}
                </span>
              </div>
            ) : viewMode === "list" ? (
              <table className="fe-table">
                <thead>
                  <tr>
                    <th aria-sort={sortCol === "name" ? (sortDir === "asc" ? "ascending" : "descending") : "none"}>
                      <button
                        type="button"
                        className={`fe-sort-btn${sortCol === "name" ? " is-active" : ""}`}
                        onClick={() => handleSortClick("name")}
                      >
                        <span>Nome</span>
                        <ArrowUpDown size={11} aria-hidden="true" />
                      </button>
                    </th>
                    <th aria-sort={sortCol === "modified" ? (sortDir === "asc" ? "ascending" : "descending") : "none"}>
                      <button
                        type="button"
                        className={`fe-sort-btn${sortCol === "modified" ? " is-active" : ""}`}
                        onClick={() => handleSortClick("modified")}
                      >
                        <span>Modificado</span>
                        <ArrowUpDown size={11} aria-hidden="true" />
                      </button>
                    </th>
                    <th aria-sort={sortCol === "type" ? (sortDir === "asc" ? "ascending" : "descending") : "none"}>
                      <button
                        type="button"
                        className={`fe-sort-btn${sortCol === "type" ? " is-active" : ""}`}
                        onClick={() => handleSortClick("type")}
                      >
                        <span>Tipo</span>
                        <ArrowUpDown size={11} aria-hidden="true" />
                      </button>
                    </th>
                    <th aria-sort={sortCol === "size" ? (sortDir === "asc" ? "ascending" : "descending") : "none"}>
                      <button
                        type="button"
                        className={`fe-sort-btn is-right${sortCol === "size" ? " is-active" : ""}`}
                        onClick={() => handleSortClick("size")}
                      >
                        <span>Tamanho</span>
                        <ArrowUpDown size={11} aria-hidden="true" />
                      </button>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {visibleEntries.map(entry => {
                    const selected = activeTab.selectedPath === entry.path;
                    return (
                      <tr
                        key={entry.path}
                        tabIndex={0}
                        aria-selected={selected}
                        className={`fe-row${selected ? " is-selected" : ""}${entry.isHidden ? " is-hidden-entry" : ""}`}
                        onClick={() =>
                          updateTab(activeTab.id, prev => ({ ...prev, selectedPath: entry.path }))
                        }
                        onDoubleClick={() => void handleOpenEntry(entry)}
                        onContextMenu={e => handleEntryContextMenu(e, entry)}
                        onKeyDown={e => handleEntryKeyDown(e, entry)}
                      >
                        <td>
                          <div className="fe-cell-name">
                            <EntryGlyph entry={entry} />
                            <div>
                              <span className="fe-entry-name">{entry.name}</span>
                              {activeTab.searchResults !== null && (
                                <span className="fe-entry-subpath">{entry.path}</span>
                              )}
                            </div>
                            {entry.isSymlink && <span className="fe-badge">Link</span>}
                            {entry.isHidden && <span className="fe-badge">Oculto</span>}
                          </div>
                        </td>
                        <td className="fe-cell-mono">{formatExplorerDate(entry.modifiedUnixMs)}</td>
                        <td className="fe-cell-mono">{entryTypeLabel(entry)}</td>
                        <td className="fe-cell-mono fe-cell-right">
                          {entry.isDir ? "—" : formatExplorerSize(entry.sizeBytes)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            ) : (
              <div className="fe-grid" role="list">
                {visibleEntries.map(entry => {
                  const selected = activeTab.selectedPath === entry.path;
                  return (
                    <button
                      key={entry.path}
                      type="button"
                      role="listitem"
                      aria-pressed={selected}
                      className={`fe-card${selected ? " is-selected" : ""}${entry.isHidden ? " is-hidden-entry" : ""}`}
                      onClick={() =>
                        updateTab(activeTab.id, prev => ({ ...prev, selectedPath: entry.path }))
                      }
                      onDoubleClick={() => void handleOpenEntry(entry)}
                      onContextMenu={e => handleEntryContextMenu(e, entry)}
                      onKeyDown={e => handleEntryKeyDown(e, entry)}
                    >
                      <EntryGlyph entry={entry} size={28} />
                      <span className="fe-card-name" title={entry.name}>
                        {entry.name}
                      </span>
                      <span className="fe-card-meta">
                        {entry.isDir ? "Pasta" : formatExplorerSize(entry.sizeBytes)}
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>

          <footer className="fe-statusbar">
            <span>{visibleEntries.length} item(ns) exibido(s)</span>
            <span>{activeTab.currentPath || "—"}</span>
          </footer>
        </main>

        {/* Painel de Preview Lateral Recolhível */}
        {previewOpen && (
          <aside className="fe-preview" aria-label="Pré-visualização do item">
            <div className="fe-preview-header">
              <h3>Pré-visualização</h3>
              <button
                type="button"
                className="fe-path-edit-btn"
                aria-label="Fechar painel de pré-visualização"
                onClick={() => setPreviewOpen(false)}
              >
                <X size={14} aria-hidden="true" />
              </button>
            </div>

            {!selectedEntry ? (
              <p className="fe-preview-note">
                Selecione um arquivo ou pasta para inspecionar metadados e trechos de texto.
              </p>
            ) : previewLoading ? (
              <p className="fe-preview-note" role="status">
                Carregando pré-visualização…
              </p>
            ) : previewError ? (
              <p className="fe-preview-note" role="alert">
                {previewError}
              </p>
            ) : preview ? (
              <>
                <div className="fe-preview-hero">
                  <EntryGlyph entry={selectedEntry} size={22} />
                  <div>
                    <strong>{preview.name}</strong>
                    <span>{entryTypeLabel(selectedEntry)}</span>
                  </div>
                </div>

                {preview.kind === "image" && preview.imageDataUrl && (
                  <div className="fe-preview-image-wrap">
                    <img src={preview.imageDataUrl} alt={`Pré-visualização de ${preview.name}`} />
                  </div>
                )}

                {preview.kind === "text" && preview.textSnippet != null && (
                  <>
                    <pre className="fe-preview-code">
                      <code>
                        {preview.textSnippet === "" ? "(Arquivo vazio)" : preview.textSnippet}
                      </code>
                    </pre>
                    {preview.textTruncated && (
                      <p className="fe-preview-note">
                        Exibindo os primeiros 64 KB do arquivo.
                      </p>
                    )}
                  </>
                )}

                {preview.note && <p className="fe-preview-note">{preview.note}</p>}

                <dl className="fe-preview-meta">
                  <dt>Tamanho</dt>
                  <dd>{preview.isDir ? "Pasta" : formatExplorerSize(preview.sizeBytes)}</dd>
                  <dt>Modificado</dt>
                  <dd>{formatExplorerDate(preview.modifiedUnixMs)}</dd>
                  <dt>Criado</dt>
                  <dd>{formatExplorerDate(preview.createdUnixMs)}</dd>
                  <dt>Atributos</dt>
                  <dd>
                    {[
                      preview.readonly ? "Somente leitura" : "Leitura/Escrita",
                      preview.isHidden ? "Oculto" : null,
                      preview.isSymlink ? "Link simbólico" : null,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </dd>
                  <dt>Caminho</dt>
                  <dd>{preview.path}</dd>
                </dl>
              </>
            ) : null}
          </aside>
        )}
      </div>

      {/* Menu de Contexto */}
      {contextMenu && (
        <div
          className="fe-context-menu"
          role="menu"
          aria-label={`Ações para ${contextMenu.entry.name}`}
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onMouseDown={e => e.stopPropagation()}
        >
          <button
            type="button"
            role="menuitem"
            className="fe-context-item"
            onClick={() => {
              const target = contextMenu.entry;
              setContextMenu(null);
              void handleOpenEntry(target);
            }}
          >
            <ExternalLink size={14} aria-hidden="true" />
            <span>{contextMenu.entry.isDir ? "Abrir pasta" : "Abrir arquivo"}</span>
          </button>
          {contextMenu.entry.isDir && (
            <button
              type="button"
              role="menuitem"
              className="fe-context-item"
              onClick={() => {
                const target = contextMenu.entry;
                setContextMenu(null);
                void handleOpenEntry(target, true);
              }}
            >
              <FolderOpen size={14} aria-hidden="true" />
              <span>Abrir no gerenciador do sistema</span>
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            className="fe-context-item"
            onClick={() => {
              const target = contextMenu.entry;
              setContextMenu(null);
              void handleOpenInTerminal(target.path);
            }}
          >
            <Terminal size={14} aria-hidden="true" />
            <span>Abrir no Terminal</span>
          </button>
          <button
            type="button"
            role="menuitem"
            className="fe-context-item"
            onClick={() => {
              const target = contextMenu.entry;
              setContextMenu(null);
              void handleCopyPath(target.path);
            }}
          >
            <Copy size={14} aria-hidden="true" />
            <span>Copiar caminho</span>
          </button>
          {contextMenu.entry.isDir && (
            <button
              type="button"
              role="menuitem"
              className="fe-context-item"
              onClick={() => {
                const target = contextMenu.entry;
                setContextMenu(null);
                void handleTogglePin(target.path);
              }}
            >
              <Pin size={14} aria-hidden="true" />
              <span>
                {quickAccess && isPathPinned(quickAccess.pins, contextMenu.entry.path)
                  ? "Desafixar pasta"
                  : "Fixar pasta"}
              </span>
            </button>
          )}
          <div className="fe-context-divider" role="separator" />
          <button
            type="button"
            role="menuitem"
            className="fe-context-item"
            onClick={() => {
              const target = contextMenu.entry;
              setContextMenu(null);
              setDialogError(null);
              setDialog({ kind: "rename", entry: target, name: target.name });
            }}
          >
            <Pencil size={14} aria-hidden="true" />
            <span>Renomear</span>
          </button>
          <button
            type="button"
            role="menuitem"
            className="fe-context-item is-danger"
            onClick={() => {
              const target = contextMenu.entry;
              setContextMenu(null);
              setDialogError(null);
              setDialog({ kind: "trash", entry: target });
            }}
          >
            <Trash2 size={14} aria-hidden="true" />
            <span>Mover para Lixeira</span>
          </button>
        </div>
      )}

      {/* Modais leves: Nova Pasta, Renomear, Confirmar Lixeira */}
      {dialog && (
        <div
          className="fe-modal-backdrop"
          onClick={() => {
            if (!dialogBusy) setDialog(null);
          }}
        >
          <div
            className="fe-modal-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="fe-dialog-title"
            onClick={e => e.stopPropagation()}
          >
            {dialog.kind === "createFolder" && (
              <>
                <h3 id="fe-dialog-title">Criar nova pasta</h3>
                <p>Será criada dentro de: {activeTab.currentPath}</p>
                <input
                  ref={modalInputRef}
                  type="text"
                  className="fe-modal-input"
                  aria-label="Nome da nova pasta"
                  placeholder="Nome da pasta"
                  value={dialog.name}
                  disabled={dialogBusy}
                  onChange={e => setDialog({ ...dialog, name: e.target.value })}
                  onKeyDown={e => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void submitDialog();
                    } else if (e.key === "Escape" && !dialogBusy) {
                      setDialog(null);
                    }
                  }}
                />
                {dialogError && (
                  <p className="fe-banner is-error" role="alert">
                    {dialogError}
                  </p>
                )}
                <div className="fe-modal-actions">
                  <button
                    type="button"
                    className="fe-pill-btn"
                    disabled={dialogBusy}
                    onClick={() => setDialog(null)}
                  >
                    Cancelar
                  </button>
                  <button
                    type="button"
                    className="fe-pill-btn is-accent"
                    disabled={dialogBusy || dialog.name.trim() === ""}
                    onClick={() => void submitDialog()}
                  >
                    {dialogBusy ? "Criando…" : "Criar pasta"}
                  </button>
                </div>
              </>
            )}

            {dialog.kind === "rename" && (
              <>
                <h3 id="fe-dialog-title">Renomear item</h3>
                <p>Novo nome para "{dialog.entry.name}" (não sobrescreve arquivos existentes).</p>
                <input
                  ref={modalInputRef}
                  type="text"
                  className="fe-modal-input"
                  aria-label="Novo nome"
                  value={dialog.name}
                  disabled={dialogBusy}
                  onChange={e => setDialog({ ...dialog, name: e.target.value })}
                  onKeyDown={e => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void submitDialog();
                    } else if (e.key === "Escape" && !dialogBusy) {
                      setDialog(null);
                    }
                  }}
                />
                {dialogError && (
                  <p className="fe-banner is-error" role="alert">
                    {dialogError}
                  </p>
                )}
                <div className="fe-modal-actions">
                  <button
                    type="button"
                    className="fe-pill-btn"
                    disabled={dialogBusy}
                    onClick={() => setDialog(null)}
                  >
                    Cancelar
                  </button>
                  <button
                    type="button"
                    className="fe-pill-btn is-accent"
                    disabled={dialogBusy || dialog.name.trim() === ""}
                    onClick={() => void submitDialog()}
                  >
                    {dialogBusy ? "Renomeando…" : "Renomear"}
                  </button>
                </div>
              </>
            )}

            {dialog.kind === "trash" && (
              <>
                <h3 id="fe-dialog-title">Mover para a Lixeira</h3>
                <p>
                  Deseja mover <strong>{dialog.entry.name}</strong> para a Lixeira do sistema? O item
                  não será apagado permanentemente e poderá ser restaurado depois.
                </p>
                {dialogError && (
                  <p className="fe-banner is-error" role="alert">
                    {dialogError}
                  </p>
                )}
                <div className="fe-modal-actions">
                  <button
                    type="button"
                    className="fe-pill-btn"
                    disabled={dialogBusy}
                    onClick={() => setDialog(null)}
                  >
                    Cancelar
                  </button>
                  <button
                    type="button"
                    className="fe-pill-btn fe-btn-danger-fill"
                    disabled={dialogBusy}
                    onClick={() => void submitDialog()}
                  >
                    {dialogBusy ? "Movendo…" : "Mover para Lixeira"}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
