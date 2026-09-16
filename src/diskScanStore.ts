import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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

export type DiskAnalyzerSharedState = {
  drives: DriveInfo[];
  selectedPath: string;
  scanResult: DiskItemNode | null;
  cachedScan: CachedDiskScan | null;
  isScanning: boolean;
  progress: DiskScanProgress | null;
  isRestoringScan: boolean;
  error: string | null;
};

export const DEFAULT_DISK_PATH = "C:\\";

const createInitialState = (): DiskAnalyzerSharedState => ({
  drives: [],
  selectedPath: DEFAULT_DISK_PATH,
  scanResult: null,
  cachedScan: null,
  isScanning: false,
  progress: null,
  isRestoringScan: true,
  error: null,
});

// Estado em escopo de módulo: sobrevive à troca de abas (o DiskAnalyzer é
// desmontado ao sair de Armazenamento) e evita reler o cache a cada abertura.
let state = createInitialState();
let userInitiatedScan = false;
let userSelectedPath = false;
let bootstrapPromise: Promise<void> | null = null;
let progressListenerPromise: Promise<void> | null = null;
let removeProgressListener: (() => void) | null = null;
const listeners = new Set<() => void>();

export function subscribeDiskAnalyzer(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getDiskAnalyzerState(): DiskAnalyzerSharedState {
  return state;
}

function patch(next: Partial<DiskAnalyzerSharedState>) {
  state = { ...state, ...next };
  for (const listener of listeners) listener();
}

export function findDiskNodeById(root: DiskItemNode, id: string): DiskItemNode | null {
  if (root.id === id) return root;
  for (const child of root.children) {
    const match = findDiskNodeById(child, id);
    if (match) return match;
  }
  return null;
}

export function setSelectedDiskPath(path: string) {
  userSelectedPath = true;
  patch({ selectedPath: path });
}

export function setDiskAnalyzerError(error: string | null) {
  patch({ error });
}

export function applyDiskScanResult(result: DiskItemNode) {
  patch({ scanResult: result });
}

export function beginUserDiskScan(path: string) {
  userInitiatedScan = true;
  patch({
    isScanning: true,
    error: null,
    scanResult: null,
    progress: {
      itemsScanned: 0,
      totalBytesScanned: 0,
      currentPath: path,
      partialResult: null,
    },
  });
}

function isTerminalProgress(progress: DiskScanProgress): boolean {
  return (
    progress.currentPath.startsWith("Varredura concluída") ||
    progress.currentPath === "Varredura cancelada"
  );
}

function applyPartialSnapshot(result: DiskItemNode) {
  // Atualização de fundo não deve substituir a última árvore visível; o
  // resultado final chega pela reconciliação e é aplicado de uma vez.
  if (!userInitiatedScan && state.scanResult) return;
  patch({ scanResult: result });
}

function handleScanProgress(progress: DiskScanProgress) {
  const finished = isTerminalProgress(progress);
  patch({ progress, isScanning: !finished });
  if (progress.partialResult) applyPartialSnapshot(progress.partialResult);
  if (finished) {
    userInitiatedScan = false;
    void reconcileDiskAnalyzer();
  }
}

function ensureProgressListener(): Promise<void> {
  if (!progressListenerPromise) {
    progressListenerPromise = listen<DiskScanProgress>(
      "pulse://disk-scan-progress",
      event => handleScanProgress(event.payload),
    )
      .then(unlisten => {
        removeProgressListener = unlisten;
      })
      .catch(() => {
        progressListenerPromise = null;
      });
  }
  return progressListenerPromise;
}

async function loadSystemDrives() {
  try {
    const result = await invoke<DriveInfo[]>("get_system_drives");
    const drives =
      Array.isArray(result) && result.length > 0
        ? result
        : [{ path: DEFAULT_DISK_PATH, label: "Disco Local (C:)" }];
    const normalize = (path: string) => path.replace(/[\\/]+$/, "").toLowerCase();
    const selectedStillAvailable = drives.some(
      drive => normalize(drive.path) === normalize(state.selectedPath),
    );
    // Sem escolha explícita do usuário, prioriza a unidade da última análise
    // exibida para que o seletor não aponte para um disco diferente da árvore.
    const currentResult = state.scanResult;
    const analyzedDrive = currentResult
      ? drives.find(drive => normalize(drive.path) === normalize(currentResult.path))
      : undefined;
    const desiredPath =
      !userSelectedPath && analyzedDrive
        ? analyzedDrive.path
        : selectedStillAvailable
          ? state.selectedPath
          : drives[0].path;
    patch({ drives, selectedPath: desiredPath });
  } catch {
    patch({
      drives: [{ path: DEFAULT_DISK_PATH, label: "Disco Local (C:)" }],
      selectedPath: DEFAULT_DISK_PATH,
    });
  }
}

export async function reconcileDiskAnalyzer(): Promise<void> {
  try {
    const scanState = await invoke<DiskScanState>("get_disk_scan_state");
    if (scanState?.error) patch({ error: scanState.error });

    if (scanState?.isScanning) {
      patch({
        isScanning: true,
        ...(scanState.progress ? { progress: scanState.progress } : {}),
      });
      if (scanState.progress?.partialResult) {
        applyPartialSnapshot(scanState.progress.partialResult);
      }
      return;
    }

    const cache = await invoke<CachedDiskScan | null>("get_cached_disk_scan");
    const next: Partial<DiskAnalyzerSharedState> = {
      isScanning: false,
      cachedScan: cache?.result ? cache : null,
    };
    if (scanState?.result) next.scanResult = scanState.result;
    else if (cache?.result) next.scanResult = cache.result;
    patch(next);
    userInitiatedScan = false;
  } catch {
    // Reconciliar é best-effort; o fluxo principal segue disponível.
  }
}

export function ensureDiskAnalyzerLoaded(): Promise<void> {
  if (!bootstrapPromise) {
    bootstrapPromise = (async () => {
      void ensureProgressListener();
      void loadSystemDrives();
      try {
        await reconcileDiskAnalyzer();
      } finally {
        patch({ isRestoringScan: false });
      }
    })();
  }
  return bootstrapPromise;
}

export function resetDiskAnalyzerStateForTests() {
  removeProgressListener?.();
  removeProgressListener = null;
  bootstrapPromise = null;
  progressListenerPromise = null;
  userInitiatedScan = false;
  userSelectedPath = false;
  state = createInitialState();
  for (const listener of listeners) listener();
}
