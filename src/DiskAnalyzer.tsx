import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useRef, useState } from "react";
import { formatFileDate, formatFileSize } from "./App";
import { FileIcon, ModuleIcon } from "./Identity";

export type DriveInfo = {
  path: string;
  label: string;
  totalBytes?: number | null;
  freeBytes?: number | null;
};

export type DiskItemNode = {
  id: string;
  name: string;
  path: string;
  sizeBytes: number;
  fileCount: number;
  dirCount: number;
  isDir: boolean;
  modifiedAtUnixMs?: number | null;
  children: DiskItemNode[];
};

export type DiskScanProgress = {
  itemsScanned: number;
  totalBytesScanned: number;
  currentPath: string;
  partialResult?: DiskItemNode | null;
};

export type DiskScanState = {
  isScanning: boolean;
  progress?: DiskScanProgress | null;
  result?: DiskItemNode | null;
  error?: string | null;
};

export type CachedDiskScan = {
  result: DiskItemNode;
  completedAtUnixMs: number;
  durationMs: number;
  isPartial?: boolean;
  itemsScanned?: number;
  stopReason?: "itemLimit" | "timeout" | "cancelled" | null;
};

function describeScanStop(cache: CachedDiskScan): string | null {
  if (!cache.isPartial) return null;
  if (cache.stopReason === "itemLimit") return "limite protetivo de 2.500 itens atingido";
  if (cache.stopReason === "timeout") return "limite protetivo de 1,5 s atingido";
  if (cache.stopReason === "cancelled") return "interrompida pelo usuário";
  return "amostra limitada para proteger a responsividade do Windows";
}

type ViewMode = "split" | "tree" | "treemap";
type TypeFilter = "all" | "dirs" | "files";

function formatScanDuration(durationMs: number): string {
  if (!Number.isFinite(durationMs) || durationMs < 0) return "duração indisponível";
  if (durationMs < 1000) return "menos de 1 s";
  const seconds = Math.round(durationMs / 1000);
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} min ${seconds % 60}s`;
}

export function formatElapsedSince(timestampMs: number, nowMs = Date.now()): string {
  if (!Number.isFinite(timestampMs) || timestampMs <= 0) return "data indisponível";
  const elapsedMs = Math.max(0, nowMs - timestampMs);
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 1) return "agora mesmo";
  if (minutes < 60) return `há ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `há ${hours} h`;
  const days = Math.floor(hours / 24);
  return `há ${days} ${days === 1 ? "dia" : "dias"}`;
}

// Paleta industrial com alto contraste para níveis de profundidade
const DEPTH_COLORS = [
  "#283530", // Nível 0
  "#34463f", // Nível 1
  "#41584f", // Nível 2
  "#4d695e", // Nível 3
  "#5a7b6e", // Nível 4
  "#688d7f", // Nível 5
  "#77a090", // Nível 6
  "#dda76d", // Destaque para níveis profundos ou itens massivos
];

type Rect = {
  x: number;
  y: number;
  w: number;
  h: number;
};

type TreemapBox = {
  node: DiskItemNode;
  rect: Rect;
  depth: number;
};

function computeTreemapBoxes(
  node: DiskItemNode,
  bounds: Rect,
  depth = 0,
  maxDepth = 8,
): TreemapBox[] {
  if (bounds.w <= 4 || bounds.h <= 4) return [];

  const result: TreemapBox[] = [];
  const children = node.children.filter(c => c.sizeBytes > 0);

  if (children.length === 0 || depth >= maxDepth) {
    result.push({ node, rect: bounds, depth });
    return result;
  }

  const totalSize = children.reduce((sum, c) => sum + c.sizeBytes, 0);
  if (totalSize <= 0) {
    result.push({ node, rect: bounds, depth });
    return result;
  }

  let remaining = { ...bounds };
  const isHorizontal = remaining.w >= remaining.h;

  let accumulated = 0;
  for (let i = 0; i < children.length; i++) {
    const child = children[i];
    let childRect: Rect;

    if (isHorizontal) {
      const width =
        i === children.length - 1
          ? remaining.w
          : Math.max(1, remaining.w * (child.sizeBytes / (totalSize - accumulated)));
      childRect = {
        x: remaining.x,
        y: remaining.y,
        w: width,
        h: remaining.h,
      };
      remaining.x += width;
      remaining.w = Math.max(0, remaining.w - width);
    } else {
      const height =
        i === children.length - 1
          ? remaining.h
          : Math.max(1, remaining.h * (child.sizeBytes / (totalSize - accumulated)));
      childRect = {
        x: remaining.x,
        y: remaining.y,
        w: remaining.w,
        h: height,
      };
      remaining.y += height;
      remaining.h = Math.max(0, remaining.h - height);
    }
    accumulated += child.sizeBytes;

    if (
      child.isDir &&
      child.children.length > 0 &&
      depth + 1 < maxDepth &&
      childRect.w > 30 &&
      childRect.h > 30
    ) {
      const padding = 2;
      const innerBounds: Rect = {
        x: childRect.x + padding,
        y: childRect.y + padding,
        w: Math.max(0, childRect.w - padding * 2),
        h: Math.max(0, childRect.h - padding * 2),
      };
      const subBoxes = computeTreemapBoxes(child, innerBounds, depth + 1, maxDepth);
      result.push(...subBoxes);
    } else {
      result.push({ node: child, rect: childRect, depth: depth + 1 });
    }
  }

  return result;
}

// Coleta os maiores arquivos de toda a árvore para a visualização "Apenas Arquivos"
export function extractTopFiles(root: DiskItemNode, limit = 60): DiskItemNode[] {
  const files: DiskItemNode[] = [];
  function traverse(n: DiskItemNode) {
    if (!n.isDir && !n.id.includes("::[")) {
      files.push(n);
    } else {
      for (const child of n.children) {
        traverse(child);
      }
    }
  }
  traverse(root);
  files.sort((a, b) => b.sizeBytes - a.sizeBytes);
  return files.slice(0, limit);
}

function isAggregateNode(node: DiskItemNode): boolean {
  return node.id.includes("::[");
}

function findNodeById(root: DiskItemNode, id: string): DiskItemNode | null {
  if (root.id === id) return root;
  for (const child of root.children) {
    const match = findNodeById(child, id);
    if (match) return match;
  }
  return null;
}

export function getFileExtension(path: string): string {
  const parts = path.split(/[/\\]/);
  const name = parts[parts.length - 1] || "";
  const dot = name.lastIndexOf(".");
  if (dot > 0 && dot < name.length - 1) {
    return name.slice(dot + 1).toUpperCase();
  }
  return "";
}

export function DiskAnalyzer({
  overviewDiskUsedGb,
  overviewDiskTotalGb,
}: {
  overviewDiskUsedGb?: number | null;
  overviewDiskTotalGb?: number | null;
} = {}) {
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [selectedPath, setSelectedPath] = useState<string>("C:\\");
  const [isScanning, setIsScanning] = useState(false);
  const [progress, setProgress] = useState<DiskScanProgress | null>(null);
  const [scanResult, setScanResult] = useState<DiskItemNode | null>(null);
  const [cachedScan, setCachedScan] = useState<CachedDiskScan | null>(null);
  const [isRestoringScan, setIsRestoringScan] = useState(true);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [error, setError] = useState<string | null>(null);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [selectedNode, setSelectedNode] = useState<DiskItemNode | null>(null);
  const [focusedTreeNode, setFocusedTreeNode] = useState<DiskItemNode | null>(null);
  const [treemapFocus, setTreemapFocus] = useState<DiskItemNode | null>(null);
  const [viewMode, setViewMode] = useState<ViewMode>("split");
  const [typeFilter, setTypeFilter] = useState<TypeFilter>("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [hoveredNode, setHoveredNode] = useState<{ node: DiskItemNode; x: number; y: number } | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const treemapContainerRef = useRef<HTMLDivElement>(null);
  const [containerDimensions, setContainerDimensions] = useState({ width: 600, height: 400 });

  const selectedDrive = useMemo(() => {
    const normalizedSelection = selectedPath.replace(/[\\/]+$/, "").toLowerCase();
    return drives.find(drive => {
      const normalizedDrive = drive.path.replace(/[\\/]+$/, "").toLowerCase();
      return normalizedSelection === normalizedDrive || normalizedSelection.startsWith(`${normalizedDrive}\\`);
    });
  }, [drives, selectedPath]);

  const driveUsedBytes = selectedDrive && typeof selectedDrive.totalBytes === "number" && typeof selectedDrive.freeBytes === "number" && selectedDrive.totalBytes >= selectedDrive.freeBytes
    ? selectedDrive.totalBytes - selectedDrive.freeBytes
    : undefined;
  const overviewMatchesSelectedDrive = selectedDrive?.path.toLowerCase().startsWith("c:") === true;
  const capacityUsedBytes = overviewMatchesSelectedDrive && typeof overviewDiskUsedGb === "number" && Number.isFinite(overviewDiskUsedGb)
    ? Math.max(0, overviewDiskUsedGb * 1024 * 1024 * 1024)
    : driveUsedBytes;
  const capacityTotalBytes = overviewMatchesSelectedDrive && typeof overviewDiskTotalGb === "number" && Number.isFinite(overviewDiskTotalGb)
    ? Math.max(0, overviewDiskTotalGb * 1024 * 1024 * 1024)
    : selectedDrive?.totalBytes;
  const capacityFreeBytes = capacityTotalBytes != null && capacityUsedBytes != null
    ? Math.max(0, capacityTotalBytes - capacityUsedBytes)
    : selectedDrive?.freeBytes;
  const driveUsedPercent = capacityTotalBytes != null && capacityTotalBytes > 0 && capacityUsedBytes != null
    ? Math.round((capacityUsedBytes / capacityTotalBytes) * 100)
    : undefined;
  const driveHealth = driveUsedPercent == null
    ? "unknown"
    : driveUsedPercent >= 95
      ? "critical"
      : driveUsedPercent >= 85
        ? "warning"
        : "normal";

  const applyScanResult = (result: DiskItemNode) => {
    setScanResult(result);
    setSelectedNode(result);
    setFocusedTreeNode(result);
    setTreemapFocus(result);
    setExpandedIds(new Set([result.id]));
  };

  const applyPartialScanResult = (result: DiskItemNode) => {
    setScanResult(result);
    setSelectedNode(previous => previous ? findNodeById(result, previous.id) ?? result : result);
    setFocusedTreeNode(previous => previous ? findNodeById(result, previous.id) ?? result : result);
    setTreemapFocus(previous => previous ? findNodeById(result, previous.id) ?? result : result);
    setExpandedIds(previous => {
      if (previous.has(result.id)) return previous;
      const next = new Set(previous);
      next.add(result.id);
      return next;
    });
  };

  const syncScanState = async () => {
    try {
      const state = await invoke<DiskScanState>("get_disk_scan_state");
      if (!state) return;
      setIsScanning(state.isScanning === true);
      if (state.progress) {
        setProgress(state.progress);
        if (state.progress.partialResult) applyPartialScanResult(state.progress.partialResult);
      }
      if (state.result) applyScanResult(state.result);
      if (state.error) setError(state.error);
    } catch {
      // A consulta é apenas uma reconciliação; o fluxo principal continua disponível.
    }
  };

  useEffect(() => {
    let unmounted = false;
    void invoke<DiskScanState>("get_disk_scan_state")
      .then(async state => {
        if (unmounted) return;
        if (state?.isScanning) {
          setCachedScan(null);
          setIsScanning(true);
          if (state.progress) {
            setProgress(state.progress);
            if (state.progress.partialResult) {
              applyPartialScanResult(state.progress.partialResult);
            }
          }
          return;
        }

        const cache = await invoke<CachedDiskScan | null>("get_cached_disk_scan");
        if (!unmounted && cache?.result) {
          setCachedScan(cache);
          applyScanResult(cache.result);
        }
      })
      .catch(() => undefined)
      .finally(() => {
        if (!unmounted) setIsRestoringScan(false);
      });
    return () => { unmounted = true; };
  }, []);

  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  // Carrega apenas as unidades disponíveis para a análise de capacidade.
  useEffect(() => {
    let unmounted = false;
    void invoke<DriveInfo[]>("get_system_drives")
      .then(res => {
        if (!unmounted) {
          if (Array.isArray(res) && res.length > 0) {
            setDrives(res);
            setSelectedPath(res[0].path);
          } else {
            setDrives([{ path: "C:\\", label: "Disco Local (C:)" }]);
            setSelectedPath("C:\\");
          }
        }
      })
      .catch(() => {
        if (!unmounted) {
          setDrives([{ path: "C:\\", label: "Disco Local (C:)" }]);
          setSelectedPath("C:\\");
        }
      });

    return () => {
      unmounted = true;
    };
  }, []);

  // Evento de progresso da varredura em tempo real
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    const setupProgressListener = async () => {
      try {
        const removeListener = await listen<DiskScanProgress>("pulse://disk-scan-progress", event => {
          setProgress(event.payload);
          if (event.payload.partialResult) {
            applyPartialScanResult(event.payload.partialResult);
          }
          if (event.payload.currentPath.startsWith("Varredura concluída") || event.payload.currentPath === "Varredura cancelada") {
            setIsScanning(false);
            void syncScanState();
          } else {
            setIsScanning(true);
          }
        });

        if (disposed) {
          removeListener();
          return;
        }
        unlisten = removeListener;
      } catch {
        // A reconciliação abaixo ainda recupera o estado atual sem eventos.
      }

      if (!disposed) {
        await syncScanState();
      }
    };

    void setupProgressListener();

    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  // Observador de tamanho do Treemap usando ResizeObserver sem timers pendentes
  useEffect(() => {
    const el = treemapContainerRef.current;
    if (!el) return;

    const measure = () => {
      const rect = el.getBoundingClientRect();
      if (rect.width > 20 && rect.height > 20) {
        setContainerDimensions({ width: Math.round(rect.width), height: Math.round(rect.height) });
      }
    };

    measure();

    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(() => measure());
      ro.observe(el);
      return () => ro.disconnect();
    }
  }, [viewMode, scanResult]);

  const handleStartScan = async () => {
    if (!selectedPath) return;
    setIsScanning(true);
    setError(null);
    setScanResult(null);
    setSelectedNode(null);
    setFocusedTreeNode(null);
    setTreemapFocus(null);
    setExpandedIds(new Set());
    setProgress({ itemsScanned: 0, totalBytesScanned: 0, currentPath: selectedPath, partialResult: null });

    try {
      const result = await invoke<DiskItemNode>("start_disk_scan", {
        path: selectedPath,
        maxDepth: 64,
      });
      applyScanResult(result);
      // O backend acabou de gravar o cache e é a fonte do horário e da duração
      // reais; o fallback mantém a identificação clara mesmo se a releitura falhar.
      const persisted = await invoke<CachedDiskScan | null>("get_cached_disk_scan");
      setCachedScan(persisted?.result
        ? persisted
        : { result, completedAtUnixMs: Date.now(), durationMs: 0 });
    } catch (err) {
      setError(typeof err === "string" ? err : "Falha ao escanear o disco.");
      void syncScanState();
    } finally {
      setIsScanning(false);
    }
  };

  const handleCancelScan = async () => {
    try {
      await invoke("cancel_disk_scan");
    } catch {
      // Ignora erro
    }
    void syncScanState();
  };

  const toggleExpand = (id: string) => {
    setExpandedIds(prev => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const handleExpandAll = () => {
    if (!scanResult) return;
    const all = new Set<string>();
    const collectIds = (n: DiskItemNode, d: number) => {
      if (d < 64) {
        all.add(n.id);
        n.children.forEach(c => collectIds(c, d + 1));
      }
    };
    collectIds(focusedTreeNode || scanResult, 0);
    setExpandedIds(all);
  };

  const handleCollapseAll = () => {
    const root = focusedTreeNode || scanResult;
    if (!root) return;
    setExpandedIds(new Set([root.id]));
  };

  const handleOpenFile = async (path: string, reveal = false) => {
    try {
      await invoke("open_disk_item", { path, reveal });
    } catch (err) {
      setError(typeof err === "string" ? err : "Não foi possível abrir o item.");
    }
  };

  const handleCopyPath = (path: string, id: string) => {
    void navigator.clipboard.writeText(path).then(() => {
      setCopiedId(id);
      setTimeout(() => setCopiedId(null), 2000);
    });
  };

  // Caixas do Treemap calculadas sobre o nó focado
  const treemapBoxes = useMemo(() => {
    const root = treemapFocus || scanResult;
    if (!root || root.sizeBytes <= 0) return [];
    return computeTreemapBoxes(
      root,
      { x: 0, y: 0, w: containerDimensions.width, h: containerDimensions.height },
      0,
      3,
    );
  }, [treemapFocus, scanResult, containerDimensions]);

  // Maiores arquivos individuais extraídos para a visão de arquivos pesados
  const topHeavyFiles = useMemo(() => {
    if (!scanResult) return [];
    const root = focusedTreeNode || scanResult;
    return extractTopFiles(root, 100);
  }, [scanResult, focusedTreeNode]);

  // Trilha de navegação (Breadcrumbs) da árvore e do treemap
  const treeBreadcrumbs = useMemo(() => {
    if (!scanResult || !focusedTreeNode) return [];
    const trail: DiskItemNode[] = [];
    const findTrail = (current: DiskItemNode, targetId: string): boolean => {
      trail.push(current);
      if (current.id === targetId) return true;
      for (const child of current.children) {
        if (findTrail(child, targetId)) return true;
      }
      trail.pop();
      return false;
    };
    findTrail(scanResult, focusedTreeNode.id);
    return trail;
  }, [scanResult, focusedTreeNode]);

  const totalRootBytes = scanResult?.sizeBytes || 1;
  const currentTreeRoot = focusedTreeNode || scanResult;

  // Renderização da Árvore Hierárquica
  const renderTreeNodes = (nodes: DiskItemNode[], level = 0): React.ReactNode => {
    return nodes
      .filter(n => {
        if (typeFilter === "dirs" && !n.isDir) return false;
        if (!searchQuery) return true;
        return (
          n.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
          n.path.toLowerCase().includes(searchQuery.toLowerCase())
        );
      })
      .map(node => {
        const isExpanded = expandedIds.has(node.id);
        const hasChildren = node.children && node.children.length > 0;
        const percent = Math.min(100, (node.sizeBytes / totalRootBytes) * 100);
        const isSelected = selectedNode?.id === node.id;
        const isAggregate = isAggregateNode(node);
        const ext = !node.isDir && !isAggregate ? getFileExtension(node.path) : "";

        return (
          <div key={node.id} className="tree-node-wrapper">
            <div
              className={`tree-row ${isSelected ? "selected" : ""} ${node.isDir ? "is-dir" : "is-file"}`}
              onClick={() => {
                setSelectedNode(node);
                if (node.isDir) {
                  setTreemapFocus(node);
                }
              }}
              onDoubleClick={() => {
                if (node.isDir && hasChildren) {
                  toggleExpand(node.id);
                } else if (!isAggregate) {
                  void handleOpenFile(node.path, true);
                }
              }}
              style={{ paddingLeft: `${14 + level * 18}px` }}
            >
              <div className="tree-cell-name">
                {hasChildren ? (
                  <button
                    type="button"
                    className="tree-chevron-btn"
                    onClick={e => {
                      e.stopPropagation();
                      toggleExpand(node.id);
                    }}
                    aria-label={isExpanded ? "Recolher pasta" : "Expandir pasta"}
                  >
                    {isExpanded ? "▼" : "▶"}
                  </button>
                ) : (
                  <span className="tree-chevron-spacer" />
                )}
                <span className="tree-icon" aria-hidden="true">
                  <FileIcon folder={node.isDir} />
                </span>
                <span className="tree-name-text" title={node.path}>
                  {node.name}
                </span>
                {/* Badge com a extensão se for arquivo pesado */}
                {!node.isDir && ext && (
                  <span className="tree-ext-badge">{ext}</span>
                )}
              </div>

              {/* Coluna Tipo com badge explícito */}
              <div className="tree-cell-type">
                {isAggregate ? (
                  <span className="type-badge type-file">GRUPO</span>
                ) : node.isDir ? (
                  <span className="type-badge type-folder">PASTA</span>
                ) : (
                  <span className="type-badge type-file">ARQUIVO</span>
                )}
              </div>

              <div className="tree-cell-size font-mono">
                {formatFileSize(node.sizeBytes)}
              </div>

              <div className="tree-cell-percent">
                <div className="tree-percent-bar-bg" title={`${percent.toFixed(1)}% do total`}>
                  <div
                    className={`tree-percent-bar-fill ${percent > 15 ? "heavy-fill" : ""}`}
                    style={{ width: `${percent.toFixed(1)}%` }}
                  />
                </div>
                <span className="tree-percent-text font-mono">
                  {percent >= 0.1 ? `${percent.toFixed(1).replace(".", ",")}%` : "< 0,1%"}
                </span>
              </div>

              <div className="tree-cell-count font-mono" title={`${node.fileCount} arquivos`}>
                {node.fileCount.toLocaleString("pt-BR")}
              </div>

              <div className="tree-cell-count font-mono" title={`${node.dirCount} pastas`}>
                {node.isDir ? node.dirCount.toLocaleString("pt-BR") : "—"}
              </div>

              <div
                className="tree-cell-date"
                title={node.modifiedAtUnixMs ? new Date(node.modifiedAtUnixMs).toLocaleString("pt-BR") : ""}
              >
                {node.modifiedAtUnixMs ? formatFileDate(node.modifiedAtUnixMs) : "—"}
              </div>

              <div className="tree-cell-actions">
                {node.isDir && node.id !== currentTreeRoot?.id && (
                  <button
                    type="button"
                    className="tree-action-btn focus-action"
                    title="Entrar/focar nesta pasta na árvore"
                    onClick={e => {
                      e.stopPropagation();
                      setFocusedTreeNode(node);
                      setTreemapFocus(node);
                      setExpandedIds(new Set([node.id]));
                    }}
                  >
                    Focar
                  </button>
                )}
                {!isAggregate && <button
                  type="button"
                  className="tree-action-btn"
                  title="Abrir no Explorador de Arquivos do Windows"
                  onClick={e => {
                    e.stopPropagation();
                    void handleOpenFile(node.path, true);
                  }}
                >
                  Explorar
                </button>}
                {!isAggregate && <button
                  type="button"
                  className="tree-action-btn"
                  title="Copiar caminho completo"
                  onClick={e => {
                    e.stopPropagation();
                    handleCopyPath(node.path, node.id);
                  }}
                >
                  {copiedId === node.id ? "Copiado!" : "Copiar"}
                </button>}
              </div>
            </div>

            {isExpanded && hasChildren && (
              <div className="tree-children-container">
                {renderTreeNodes(node.children, level + 1)}
              </div>
            )}
          </div>
        );
      });
  };

  // Renderização do ranking de Maiores Arquivos (Top Heavy Files)
  const renderTopFilesTable = () => {
    const filtered = topHeavyFiles.filter(f => {
      if (!searchQuery) return true;
      return (
        f.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
        f.path.toLowerCase().includes(searchQuery.toLowerCase())
      );
    });

    if (filtered.length === 0) {
      return (
        <div className="disk-empty-state">
          <p className="muted">Nenhum arquivo individual encontrado com os filtros aplicados.</p>
        </div>
      );
    }

    return filtered.map((file, rank) => {
      const percent = Math.min(100, (file.sizeBytes / totalRootBytes) * 100);
      const isSelected = selectedNode?.id === file.id;
      const ext = getFileExtension(file.path);

      return (
        <div
          key={file.id}
          className={`tree-row ${isSelected ? "selected" : ""} is-file`}
          onClick={() => setSelectedNode(file)}
          onDoubleClick={() => void handleOpenFile(file.path, true)}
          style={{ paddingLeft: "16px" }}
        >
          <div className="tree-cell-name">
            <span className="file-rank-badge font-mono">#{rank + 1}</span>
            <FileIcon />
            <span className="tree-name-text" title={file.path}>
              {file.name}
            </span>
            {ext && <span className="tree-ext-badge">{ext}</span>}
          </div>

          <div className="tree-cell-type">
            <span className="type-badge type-file">ARQUIVO</span>
          </div>

          <div className="tree-cell-size font-mono heavy-text">
            {formatFileSize(file.sizeBytes)}
          </div>

          <div className="tree-cell-percent">
            <div className="tree-percent-bar-bg" title={`${percent.toFixed(1)}% do total`}>
              <div
                className="tree-percent-bar-fill heavy-fill"
                style={{ width: `${percent.toFixed(1)}%` }}
              />
            </div>
            <span className="tree-percent-text font-mono">
              {percent >= 0.1 ? `${percent.toFixed(1).replace(".", ",")}%` : "< 0,1%"}
            </span>
          </div>

          <div className="tree-cell-count font-mono">—</div>
          <div className="tree-cell-count font-mono">—</div>

          <div
            className="tree-cell-date"
            title={file.modifiedAtUnixMs ? new Date(file.modifiedAtUnixMs).toLocaleString("pt-BR") : ""}
          >
            {file.modifiedAtUnixMs ? formatFileDate(file.modifiedAtUnixMs) : "—"}
          </div>

          <div className="tree-cell-actions">
            <button
              type="button"
              className="tree-action-btn"
              title="Abrir no Explorador de Arquivos"
              onClick={e => {
                e.stopPropagation();
                void handleOpenFile(file.path, true);
              }}
            >
              Explorar
            </button>
            <button
              type="button"
              className="tree-action-btn"
              title="Copiar caminho"
              onClick={e => {
                e.stopPropagation();
                handleCopyPath(file.path, file.id);
              }}
            >
              {copiedId === file.id ? "Copiado!" : "Copiar"}
            </button>
          </div>
        </div>
      );
    });
  };

  return (
    <div className="disk-analyzer-container">
      {/* Barra de Ferramentas Superior */}
      <header className="disk-toolbar panel">
        <div className="disk-toolbar-row">
          <div className="disk-drive-picker">
            <label htmlFor="drive-select" className="disk-toolbar-label">
              UNIDADE
            </label>
            <select
              id="drive-select"
              aria-label="Selecionar unidade de disco"
              value={selectedPath}
              onChange={e => {
                setSelectedPath(e.target.value);
                setFocusedTreeNode(null);
              }}
              disabled={isScanning}
              className="disk-select font-mono"
            >
              {drives.map(d => (
                <option key={d.path} value={d.path}>
                  {d.label} {typeof d.totalBytes === "number" && d.totalBytes > 0 ? `· ${formatFileSize(d.freeBytes)} livres` : "· capacidade indisponível"}
                </option>
              ))}
            </select>
          </div>

          {selectedDrive && capacityTotalBytes != null && capacityTotalBytes > 0 && capacityUsedBytes != null && capacityFreeBytes != null && (
            <section
              className={`disk-capacity-summary disk-capacity-${driveHealth}`}
              aria-label={`Capacidade real de ${selectedDrive.path}`}
            >
              <div className="disk-capacity-summary-head">
                <div>
                  <span className="disk-capacity-kicker">CAPACIDADE</span>
                  <strong>{formatFileSize(capacityUsedBytes)} / {formatFileSize(capacityTotalBytes)}</strong>
                </div>
                <span className="disk-capacity-health">
                  <i aria-hidden="true" />
                  {formatFileSize(capacityFreeBytes)} livres
                </span>
              </div>
              <div className="disk-capacity-track" aria-hidden="true">
                <i style={{ width: `${driveUsedPercent}%` }} />
              </div>
              <div className="disk-capacity-summary-foot">
                <span>{driveUsedPercent}% OCUPADO</span>
                <span>{scanResult ? `CATALOGADO ${formatFileSize(scanResult.sizeBytes)}` : "AGUARDANDO ANÁLISE"}</span>
              </div>
            </section>
          )}

          <div className="disk-scan-actions">
            {isScanning ? (
              <button
                type="button"
                className="btn-danger disk-scan-btn"
                onClick={() => void handleCancelScan()}
              >
                <span className="scan-spinner" aria-hidden="true" />
                PARAR VARREDURA
              </button>
            ) : (
              <button
                type="button"
                className="btn-primary disk-scan-btn"
                onClick={() => void handleStartScan()}
              >
                <ModuleIcon index={3} />
                ESCANEAR DISCO
              </button>
            )}
          </div>
        </div>

        {/* Status de Varredura em Tempo Real */}
        {isScanning && progress && (
          <div className="disk-progress-bar-container" role="status" aria-live="polite">
            <div className="disk-progress-info">
              <span className="font-mono">
                Varrendo: <strong>{progress.itemsScanned.toLocaleString("pt-BR")}</strong> itens examinados
                ({formatFileSize(progress.totalBytesScanned)} calculados)
              </span>
              <span className="disk-progress-path font-mono" title={progress.currentPath}>
                {progress.currentPath}
              </span>
            </div>
            <div className="disk-progress-track">
              <div className="disk-progress-fill-animated" />
            </div>
          </div>
        )}

        {cachedScan && !isScanning && (
          <div className="disk-cache-status" role="status" aria-live="polite">
            <span>{cachedScan.isPartial ? "VARREDURA CANCELADA EM CACHE" : "RESULTADO EM CACHE"}</span>
            <strong>{formatElapsedSince(cachedScan.completedAtUnixMs, nowMs)}</strong>
            <span title={new Date(cachedScan.completedAtUnixMs).toLocaleString("pt-BR")}>
              executada em {new Date(cachedScan.completedAtUnixMs).toLocaleString("pt-BR")}
            </span>
            <span>duração: {formatScanDuration(cachedScan.durationMs)}</span>
            {typeof cachedScan.itemsScanned === "number" && cachedScan.itemsScanned > 0 && (
              <span>{cachedScan.itemsScanned.toLocaleString("pt-BR")} itens examinados</span>
            )}
            {describeScanStop(cachedScan) && <span>{describeScanStop(cachedScan)}</span>}
          </div>
        )}

        {/* Barra secundária: visualização e filtros */}
        {scanResult && !isScanning && (
          <div className="disk-subtoolbar-row">
            {/* Alternador de Modo de Janela */}
            <div className="disk-view-modes" role="group" aria-label="Modo de visualização">
              <button
                type="button"
                className={`view-mode-btn ${viewMode === "split" ? "active" : ""}`}
                onClick={() => setViewMode("split")}
                title="Visão Dividida (Árvore + Treemap)"
              >
                DIVIDIDA (TREE + MAP)
              </button>
              <button
                type="button"
                className={`view-mode-btn ${viewMode === "tree" ? "active" : ""}`}
                onClick={() => setViewMode("tree")}
                title="Janela em Árvore Hierárquica"
              >
                TREE WINDOW
              </button>
              <button
                type="button"
                className={`view-mode-btn ${viewMode === "treemap" ? "active" : ""}`}
                onClick={() => setViewMode("treemap")}
                title="Visualização em Mapa de Blocos (Treemap)"
              >
                TREEMAP COMPLETO
              </button>
            </div>

            {/* Filtro: Arquivos vs Pastas Mais Pesados */}
            <div className="disk-type-filter" role="group" aria-label="Filtro de tipo">
              <button
                type="button"
                className={`type-filter-btn ${typeFilter === "all" ? "active" : ""}`}
                onClick={() => setTypeFilter("all")}
              >
                TODOS OS ITENS
              </button>
              <button
                type="button"
                className={`type-filter-btn ${typeFilter === "dirs" ? "active" : ""}`}
                onClick={() => setTypeFilter("dirs")}
              >
                APENAS PASTAS
              </button>
              <button
                type="button"
                className={`type-filter-btn ${typeFilter === "files" ? "active" : ""}`}
                onClick={() => setTypeFilter("files")}
              >
                MAIORES ARQUIVOS
              </button>
            </div>

            <div className="disk-tools-right">
              <input
                type="text"
                aria-label="Filtrar pastas ou arquivos"
                placeholder="Buscar por nome..."
                value={searchQuery}
                onChange={e => setSearchQuery(e.target.value)}
                className="disk-filter-input"
              />
              {typeFilter !== "files" && (
                <>
                  <button
                    type="button"
                    className="text-button"
                    onClick={handleExpandAll}
                    title="Expandir primeiros níveis"
                  >
                    EXPANDIR
                  </button>
                  <button
                    type="button"
                    className="text-button"
                    onClick={handleCollapseAll}
                    title="Recolher nós"
                  >
                    RECOLHER
                  </button>
                </>
              )}
            </div>
          </div>
        )}
      </header>

      {error && (
        <div className="disk-error-banner" role="alert">
          <p>{error}</p>
          <button type="button" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {/* Área Principal de Visualização */}
      <main className={`disk-workspace mode-${viewMode}`}>
        {/* Painel da Árvore Hierárquica (Tree Window) */}
        {(viewMode === "split" || viewMode === "tree") && (
          <section
            className="disk-tree-section panel"
            aria-label="Árvore de ocupação de disco"
          >
            {/* Breadcrumbs e Navegação da Tree Window */}
            {scanResult && (
              <div className="tree-nav-bar">
                <div className="tree-nav-breadcrumbs">
                  <span className="tree-nav-label">LOCALIZAÇÃO:</span>
                  {treeBreadcrumbs.map((b, idx) => (
                    <span key={b.id} className="breadcrumb-item">
                      <button
                        type="button"
                        className={`breadcrumb-btn font-mono ${idx === treeBreadcrumbs.length - 1 ? "current" : ""}`}
                        onClick={() => {
                          setFocusedTreeNode(b);
                          setTreemapFocus(b);
                        }}
                      >
                        {b.name || b.path}
                      </button>
                      {idx < treeBreadcrumbs.length - 1 && <span className="breadcrumb-sep">/</span>}
                    </span>
                  ))}
                </div>

                {focusedTreeNode && scanResult && focusedTreeNode.id !== scanResult.id && (
                  <div className="tree-nav-actions">
                    <button
                      type="button"
                      className="text-button"
                      onClick={() => {
                        // Sobe um nível no breadcrumb
                        if (treeBreadcrumbs.length >= 2) {
                          const parent = treeBreadcrumbs[treeBreadcrumbs.length - 2];
                          setFocusedTreeNode(parent);
                          setTreemapFocus(parent);
                        } else {
                          setFocusedTreeNode(scanResult);
                          setTreemapFocus(scanResult);
                        }
                      }}
                    >
                      ⬅ SUBIR NÍVEL
                    </button>
                    <button
                      type="button"
                      className="text-button"
                      onClick={() => {
                        setFocusedTreeNode(scanResult);
                        setTreemapFocus(scanResult);
                      }}
                    >
                      VOLTAR À RAIZ
                    </button>
                  </div>
                )}
              </div>
            )}

            {/* Cabeçalho da Tree Window */}
            <div className="disk-tree-header">
              <div className="tree-col-header tree-col-name">NOME</div>
              <div className="tree-col-header tree-col-type">TIPO</div>
              <div className="tree-col-header tree-col-size">TAMANHO</div>
              <div className="tree-col-header tree-col-percent">% TOTAL</div>
              <div className="tree-col-header tree-col-count">ARQUIVOS</div>
              <div className="tree-col-header tree-col-count">PASTAS</div>
              <div className="tree-col-header tree-col-date">ÚLTIMA MODIFICAÇÃO</div>
              <div className="tree-col-header tree-col-actions">AÇÕES</div>
            </div>

            <div className="disk-tree-body">
              {isRestoringScan && !scanResult ? (
                <div className="disk-panel-loading" role="status" aria-live="polite">
                  <span className="disk-panel-loading-mark" aria-hidden="true"><ModuleIcon index={3} /></span>
                  <div>
                    <strong>CARREGANDO ANÁLISE LOCAL</strong>
                    <span>Restaurando a árvore da última varredura…</span>
                  </div>
                  <i className="disk-panel-loading-line" aria-hidden="true" />
                </div>
              ) : scanResult ? (
                typeFilter === "files" ? (
                  renderTopFilesTable()
                ) : (
                  currentTreeRoot && renderTreeNodes([currentTreeRoot])
                )
              ) : (
                <div className="disk-empty-state">
                  <div className="disk-empty-icon"><ModuleIcon index={3} /></div>
                  <h3>Nenhuma varredura realizada</h3>
                  <p>
                    Selecione uma unidade e clique em <strong>ESCANEAR DISCO</strong> para visualizar as pastas e os arquivos que mais ocupam espaço.
                  </p>
                </div>
              )}
            </div>
          </section>
        )}

        {/* Painel do Treemap (Mapa de Blocos) */}
        {(viewMode === "split" || viewMode === "treemap") && (
          <section
            className="disk-treemap-section panel"
            aria-label="Mapa de ocupação em blocos (Treemap)"
          >
            {/* Cabeçalho e Breadcrumbs do Treemap */}
            <div className="treemap-header">
              <div className="treemap-breadcrumbs" aria-label="Navegação em pastas do treemap">
                <span className="treemap-label-prefix">FOCO:</span>
                {treeBreadcrumbs.map((b, idx) => (
                  <span key={b.id} className="breadcrumb-item">
                    <button
                      type="button"
                      className={`breadcrumb-btn font-mono ${idx === treeBreadcrumbs.length - 1 ? "current" : ""}`}
                      onClick={() => {
                        setTreemapFocus(b);
                        setFocusedTreeNode(b);
                      }}
                    >
                      {b.name || b.path}
                    </button>
                    {idx < treeBreadcrumbs.length - 1 && <span className="breadcrumb-sep">/</span>}
                  </span>
                ))}
              </div>

              {treemapFocus && scanResult && treemapFocus.id !== scanResult.id && (
                <button
                  type="button"
                  className="text-button treemap-reset-btn"
                  onClick={() => {
                    setTreemapFocus(scanResult);
                    setFocusedTreeNode(scanResult);
                  }}
                >
                  RESETAR FOCO
                </button>
              )}
            </div>

            {/* Container do Treemap Interativo */}
            <div
              ref={treemapContainerRef}
              className="treemap-canvas"
              onMouseLeave={() => setHoveredNode(null)}
            >
              {isRestoringScan && !scanResult && (
                <div className="disk-panel-loading" role="status" aria-live="polite">
                  <span className="disk-panel-loading-mark" aria-hidden="true"><ModuleIcon index={3} /></span>
                  <div>
                    <strong>PREPARANDO MAPA DE BLOCOS</strong>
                    <span>Organizando os dados da análise em cache…</span>
                  </div>
                  <i className="disk-panel-loading-line" aria-hidden="true" />
                </div>
              )}
              {treemapBoxes.map((box, idx) => {
                const isDir = box.node.isDir;
                const isAggregate = isAggregateNode(box.node);
                const percent = ((box.node.sizeBytes / totalRootBytes) * 100).toFixed(1);
                const bg = DEPTH_COLORS[box.depth % DEPTH_COLORS.length];
                const canShowLabel = box.rect.w > 48 && box.rect.h > 26;

                return (
                  <div
                    key={`${box.node.id}-${idx}`}
                    className={`treemap-box ${isDir ? "is-folder" : "is-file"}`}
                    style={{
                      left: `${box.rect.x}px`,
                      top: `${box.rect.y}px`,
                      width: `${box.rect.w}px`,
                      height: `${box.rect.h}px`,
                      backgroundColor: bg,
                    }}
                    onClick={() => {
                      setSelectedNode(box.node);
                      if (box.node.isDir && box.node.children.length > 0) {
                        setTreemapFocus(box.node);
                        setFocusedTreeNode(box.node);
                      }
                    }}
                    onDoubleClick={() => {
                      if (!isAggregate) void handleOpenFile(box.node.path, true);
                    }}
                    onMouseEnter={e => {
                      const rect = e.currentTarget.getBoundingClientRect();
                      setHoveredNode({
                        node: box.node,
                        x: rect.left + rect.width / 2,
                        y: rect.top,
                      });
                    }}
                  >
                    {canShowLabel && (
                      <div className="treemap-label">
                        <span className="treemap-label-name">{box.node.name}</span>
                        <span className="treemap-label-size font-mono">
                          {formatFileSize(box.node.sizeBytes)} ({percent}%)
                        </span>
                      </div>
                    )}
                  </div>
                );
              })}

              {/* Tooltip flutuante */}
              {hoveredNode && (
                <div
                  className="treemap-tooltip"
                  style={{
                    left: `${hoveredNode.x}px`,
                    top: `${hoveredNode.y - 10}px`,
                  }}
                >
                  <p className="treemap-tooltip-title">
                    <FileIcon folder={hoveredNode.node.isDir} />
                    {hoveredNode.node.name}
                  </p>
                  <p className="treemap-tooltip-path font-mono">{hoveredNode.node.path}</p>
                  <div className="treemap-tooltip-stats font-mono">
                    <span>Tamanho: <strong>{formatFileSize(hoveredNode.node.sizeBytes)}</strong></span>
                    <span>Proporção: <strong>{((hoveredNode.node.sizeBytes / totalRootBytes) * 100).toFixed(1)}%</strong></span>
                    <span>Itens: <strong>{hoveredNode.node.fileCount.toLocaleString("pt-BR")}</strong></span>
                  </div>
                </div>
              )}
            </div>

            {/* Legenda de Níveis de Profundidade */}
            <footer className="treemap-legend">
              <span className="legend-label">NÍVEIS DE PROFUNDIDADE:</span>
              {DEPTH_COLORS.slice(0, 6).map((col, i) => (
                <div key={col} className="legend-item">
                  <span className="legend-color-chip" style={{ backgroundColor: col }} />
                  <span className="legend-text font-mono">Nível {i}</span>
                </div>
              ))}
            </footer>
          </section>
        )}
      </main>
    </div>
  );
}
