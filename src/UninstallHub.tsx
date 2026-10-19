import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  ShieldAlert,
  Undo2,
} from "lucide-react";

import "./UninstallHub.css";

export type InstalledApp = {
  id: string;
  hive: string;
  view: string;
  registryPath: string;
  displayName: string;
  publisher?: string | null;
  version?: string | null;
  installDate?: string | null;
  installLocation?: string | null;
  estimatedSizeKb?: number | null;
  uninstallString?: string | null;
  isMsi: boolean;
  icon?: string | null;
};

export type InstalledAppsSnapshot = {
  apps: InstalledApp[];
  totalCount: number;
  partialReason?: string | null;
  scannedRoots: number;
  collectedAtUnixMs: number;
};

export type RegistryKeyRef = {
  id: string;
  hive: string;
  view: string;
  path: string;
  reason: string;
  confidence: string;
  exists: boolean;
};

export type FolderNode = {
  id: string;
  name: string;
  kind: "folder" | "file";
  path: string;
  sizeBytes?: number | null;
  childCount?: number | null;
  children?: FolderNode[] | null;
  blockedReason?: string | null;
  truncated?: string | null;
};

export type UninstallTargets = {
  appId: string;
  displayName: string;
  publisher?: string | null;
  version?: string | null;
  uninstallString?: string | null;
  installLocation?: string | null;
  isMsi: boolean;
  registryKeys: RegistryKeyRef[];
  folders: FolderNode[];
  skippedReason?: string | null;
};

export type UninstallCapabilities = {
  elevated: boolean;
  restorePointSupported: boolean;
  restorePointUnavailableReason?: string | null;
  verificationAvailable: boolean;
};

export type UninstallSession = {
  token: string;
  targets: UninstallTargets;
  elevated: boolean;
  restorePointSupported: boolean;
  restorePointUnavailableReason?: string | null;
  startedAtUnixMs: number;
};

export type RestorePointOutcome = {
  created: boolean;
  verified: boolean;
  sequenceNumber: number;
  description: string;
  unavailableReason?: string | null;
};

export type UninstallerOutcome = {
  started: boolean;
  exitCode?: number | null;
  timedOut: boolean;
  entryRemoved: boolean;
  detectionNote?: string | null;
  messages: string[];
  restorePoint?: RestorePointOutcome | null;
  targets: UninstallTargets;
  finishedAtUnixMs: number;
};

export type CleanupItemResult = {
  id: string;
  path: string;
  removed: boolean;
  freedBytes?: number | null;
  message: string;
};

export type CleanupReport = {
  registrySelected: number;
  registryRemoved: number;
  folderSelected: number;
  folderRemoved: number;
  freedBytes: number;
  items: CleanupItemResult[];
  finishedAtUnixMs: number;
};

type Stage = "restore" | "running" | "registry" | "folders" | "done";

const UNAVAILABLE = "Indisponível";

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || typeof bytes !== "number" || !Number.isFinite(bytes) || bytes < 0) {
    return UNAVAILABLE;
  }
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1).replace(".", ",")} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1).replace(".", ",")} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2).replace(".", ",")} GB`;
}

export function formatEstimatedSize(kilobytes: number | null | undefined): string {
  if (kilobytes == null || typeof kilobytes !== "number" || !Number.isFinite(kilobytes) || kilobytes <= 0) {
    return `Tamanho ${UNAVAILABLE.toLowerCase()}`;
  }
  return formatBytes(kilobytes * 1024);
}

export function monogram(app: InstalledApp): string {
  const words = app.displayName.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "?";
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return `${words[0][0]}${words[1][0]}`.toUpperCase();
}

export function matchesQuery(app: InstalledApp, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return (
    app.displayName.toLowerCase().includes(needle) ||
    (app.publisher ?? "").toLowerCase().includes(needle)
  );
}

export function filterApps(apps: InstalledApp[], query: string): InstalledApp[] {
  return apps.filter(app => matchesQuery(app, query));
}

export function sortApps(apps: InstalledApp[]): InstalledApp[] {
  return [...apps].sort((left, right) =>
    left.displayName.localeCompare(right.displayName, "pt-BR", { sensitivity: "base" }),
  );
}

export function existingRegistryKeys(keys: RegistryKeyRef[]): RegistryKeyRef[] {
  return keys.filter(key => key.exists);
}

/** Todas as pastas e descendentes de um nó — a remoção é recursiva, então a
 *  seleção precisa acompanhar a árvore inteira. */
export function descendantsOf(node: FolderNode): string[] {
  return [node.path, ...(node.children ?? []).flatMap(descendantsOf)];
}

export function toggleNode(selected: Set<string>, node: FolderNode): Set<string> {
  const next = new Set(selected);
  const ids = descendantsOf(node);
  const remove = ids.some(id => next.has(id));
  for (const id of ids) {
    if (remove) next.delete(id);
    else next.add(id);
  }
  return next;
}

/** `true`, `false` ou `null` (parcial) — o tri-state do checkbox de pasta. */
export function nodeCheckState(node: FolderNode, selected: Set<string>): boolean | null {
  const ids = descendantsOf(node);
  const hits = ids.filter(id => selected.has(id)).length;
  if (hits === 0) return false;
  if (hits === ids.length) return true;
  return null;
}

/**
 * Soma sem duplicar: quando um nó está marcado, toda a subárvore abaixo dele já
 * está incluída — descer de novo contaria o mesmo arquivo duas vezes.
 */
export function selectedFoldersSize(folders: FolderNode[], selected: Set<string>): number {
  const sumBelow = (node: FolderNode): number => {
    if (selected.has(node.path)) return node.sizeBytes ?? 0;
    if (node.kind === "file") return 0;
    return (node.children ?? []).reduce((total, child) => total + sumBelow(child), 0);
  };
  return folders.reduce((total, root) => total + sumBelow(root), 0);
}

export function collectAllFolderPaths(folders: FolderNode[]): string[] {
  return folders.flatMap(descendantsOf);
}

export function canCleanRegistry(keys: RegistryKeyRef[], selected: Set<string>): boolean {
  return existingRegistryKeys(keys).some(key => selected.has(key.id));
}

export function UninstallHub() {
  const [snapshot, setSnapshot] = useState<InstalledAppsSnapshot>();
  const [loadState, setLoadState] = useState<"idle" | "loading" | "ready">("idle");
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");

  const [capabilities, setCapabilities] = useState<UninstallCapabilities>();
  const [app, setApp] = useState<InstalledApp>();
  const [session, setSession] = useState<UninstallSession>();
  const [stage, setStage] = useState<Stage>("restore");
  const [restoreRequested, setRestoreRequested] = useState(true);
  const [runningMessages, setRunningMessages] = useState<string[]>([]);
  const [outcome, setOutcome] = useState<UninstallerOutcome>();
  const [selectedRegistry, setSelectedRegistry] = useState<Set<string>>(new Set());
  const [selectedFolders, setSelectedFolders] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [cleaning, setCleaning] = useState(false);
  const [report, setReport] = useState<CleanupReport>();
  const [stageError, setStageError] = useState("");

  const loadApps = useCallback(async (forceRefresh: boolean) => {
    setLoadState(current => (current === "ready" ? current : "loading"));
    setError("");
    try {
      const next = await invoke<InstalledAppsSnapshot>("list_installed_apps", { forceRefresh });
      setSnapshot(next);
      setLoadState("ready");
    } catch (cause) {
      setError(String(cause));
      setLoadState("ready");
    }
  }, []);

  useEffect(() => {
    if (loadState !== "idle") return;
    void loadApps(false);
    invoke<UninstallCapabilities>("get_uninstall_capabilities")
      .then(setCapabilities)
      .catch(() => setCapabilities(undefined));
  }, [loadApps, loadState]);

  const apps = useMemo(
    () => sortApps(filterApps(snapshot?.apps ?? [], query)),
    [snapshot, query],
  );

  const registryCandidates = useMemo(
    () => existingRegistryKeys(session?.targets.registryKeys ?? []),
    [session],
  );
  const folderCandidates = session?.targets.folders ?? [];
  const selectedFolderBytes = useMemo(
    () => selectedFoldersSize(folderCandidates, selectedFolders),
    [folderCandidates, selectedFolders],
  );
  const canRestore = capabilities?.restorePointSupported === true;
  const restoreBlockedReason = capabilities?.restorePointUnavailableReason ?? null;

  const closeModal = useCallback(async () => {
    if (session) {
      try {
        await invoke("cancel_uninstall_session", { sessionToken: session.token });
      } catch {
        // Cancelar sessão é best-effort: ela expira sozinha em 30 minutos.
      }
    }
    setApp(undefined);
    setSession(undefined);
    setOutcome(undefined);
    setReport(undefined);
    setStage("restore");
    setStageError("");
    setSelectedRegistry(new Set());
    setSelectedFolders(new Set());
    setExpanded(new Set());
    setRunningMessages([]);
    setRestoreRequested(true);
  }, [session]);

  const openApp = async (target: InstalledApp) => {
    setStageError("");
    setError("");
    setSession(undefined);
    setOutcome(undefined);
    setReport(undefined);
    setStage("restore");
    try {
      const next = await invoke<UninstallSession>("start_uninstall_session", { appId: target.id });
      setSession(next);
      setApp(target);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const runUninstaller = async () => {
    if (!session) return;
    setStage("running");
    setStageError("");
    setRunningMessages(["O desinstalador do fabricante foi aberto. Aguarde…"]);
    try {
      const result = await invoke<UninstallerOutcome>("run_app_uninstaller", {
        sessionToken: session.token,
        createRestorePoint: restoreRequested,
      });
      setOutcome(result);
      setSession(current => (current ? { ...current, targets: result.targets } : current));
      setStage("registry");
      setRunningMessages(result.messages);
    } catch (cause) {
      setStage("registry");
      setRunningMessages(["O desinstalador não pôde ser executado."]);
      setStageError(String(cause));
    }
  };

  const cleanRegistry = async () => {
    if (!session) return;
    setCleaning(true);
    setStageError("");
    try {
      const next = await invoke<CleanupReport>("cleanup_uninstall_residue", {
        sessionToken: session.token,
        registryIds: [...selectedRegistry],
        folderPaths: [],
      });
      setReport(next);
      setSelectedRegistry(new Set());
    } catch (cause) {
      setStageError(String(cause));
    } finally {
      setCleaning(false);
    }
  };

  const cleanFolders = async () => {
    if (!session) return;
    setCleaning(true);
    setStageError("");
    try {
      const roots = new Set(folderCandidates.map(folder => folder.path));
      // Só a raiz selecionada é enviada: o backend remove a subárvore inteira e
      // receber cada descendente duplicaria a operação.
      const paths = [...selectedFolders].filter(path => roots.has(path));
      const next = await invoke<CleanupReport>("cleanup_uninstall_residue", {
        sessionToken: session.token,
        registryIds: [],
        folderPaths: paths,
      });
      setReport(next);
      setSelectedFolders(new Set());
      setStage("done");
      void loadApps(true);
    } catch (cause) {
      setStageError(String(cause));
    } finally {
      setCleaning(false);
    }
  };

  const renderTree = (node: FolderNode, depth: number) => {
    const state = nodeCheckState(node, selectedFolders);
    const isFolder = node.kind === "folder";
    const isOpen = expanded.has(node.path);
    const children = node.children ?? [];
    const blocked = node.blockedReason;
    return (
      <li key={node.path} className="un-tree-item">
        <div className="un-tree-row" style={{ paddingLeft: `${depth * 16 + 8}px` }}>
          {isFolder && children.length > 0 ? (
            <button
              type="button"
              className="un-tree-toggle"
              aria-label={isOpen ? `Recolher ${node.name}` : `Expandir ${node.name}`}
              aria-expanded={isOpen}
              onClick={() =>
                setExpanded(current => {
                  const next = new Set(current);
                  next.has(node.path) ? next.delete(node.path) : next.add(node.path);
                  return next;
                })
              }
            >
              {isOpen ? <ChevronDown size={14} aria-hidden="true" /> : <ChevronRight size={14} aria-hidden="true" />}
            </button>
          ) : (
            <span className="un-tree-toggle" aria-hidden="true" />
          )}
          {isFolder ? <FolderOpen size={15} aria-hidden="true" className="un-tree-icon" /> : <Folder size={15} aria-hidden="true" className="un-tree-icon" />}
          <label className="un-tree-label">
            <input
              type="checkbox"
              checked={state === true}
              disabled={!isFolder || Boolean(blocked)}
              onChange={() => setSelectedFolders(current => toggleNode(current, node))}
            />
            <span className="un-tree-name">{node.name}</span>
          </label>
          <span className="un-tree-size">{isFolder ? `${node.childCount ?? 0} itens` : formatBytes(node.sizeBytes)}</span>
        </div>
        {blocked && <p className="un-tree-blocked">{blocked}</p>}
        {node.truncated && <p className="un-tree-truncated">{node.truncated}</p>}
        {isOpen && children.length > 0 && (
          <ul className="un-tree-children">
            {children.map(child => renderTree(child, depth + 1))}
          </ul>
        )}
      </li>
    );
  };

  const modalTitle = app ? `Desinstalar ${app.displayName}` : "Desinstalar programa";

  return (
    <section className="un-hub" aria-label="Desinstalar programas">
      <header className="un-toolbar">
        <div>
          <strong>{snapshot?.totalCount ?? 0} programas instalados</strong>
          <small>
            {snapshot?.partialReason
              ? `Leitura parcial: ${snapshot.partialReason}`
              : "Desinstalador padrão do fabricante, com rastreamento de resíduos."}
          </small>
        </div>
        <div className="un-toolbar-actions">
          <input
            type="search"
            className="un-search"
            placeholder="Filtrar por nome ou fabricante"
            aria-label="Filtrar programas"
            value={query}
            onChange={event => setQuery(event.target.value)}
          />
          <button type="button" className="un-btn" onClick={() => void loadApps(true)} disabled={loadState === "loading"}>
            {loadState === "loading" ? "CARREGANDO…" : "ATUALIZAR"}
          </button>
        </div>
      </header>

      {error && <p className="un-alert" role="alert">{error}</p>}

      {loadState === "loading" && <p className="un-loading" role="status">Lendo o registro de programas instalados…</p>}

      <ul className="un-list" role="list">
        {apps.map(item => (
          <li key={item.id}>
            <button type="button" className="un-item" onClick={() => void openApp(item)}>
              <span className="un-monogram" aria-hidden="true">
                {item.icon ? (
                  <img src={item.icon} alt="" className="un-item-icon" />
                ) : (
                  monogram(item)
                )}
              </span>
              <span className="un-item-text">
                <strong>{item.displayName}</strong>
                <small>
                  {item.publisher ?? `Fabricante ${UNAVAILABLE.toLowerCase()}`}
                  {item.version ? ` · versão ${item.version}` : ""}
                </small>
              </span>
              <span className="un-item-meta">
                <b>{formatEstimatedSize(item.estimatedSizeKb)}</b>
                <small>{item.isMsi ? "Windows Installer" : "Desinstalador próprio"}</small>
              </span>
            </button>
          </li>
        ))}
        {loadState === "ready" && apps.length === 0 && (
          <li className="un-empty">
            {query ? "Nenhum programa corresponde ao filtro." : "Nenhum programa instalado foi encontrado neste computador."}
          </li>
        )}
      </ul>

      {app && session && (
        <div className="un-modal-backdrop">
          <div className="un-modal" role="dialog" aria-modal="true" aria-labelledby="un-modal-title">
            <header className="un-modal-header">
              <div className="un-modal-header-info">
                {app.icon && (
                  <img src={app.icon} alt="" className="un-modal-app-icon" aria-hidden="true" />
                )}
                <div>
                  <span className="eyebrow">
                    ETAPA {stage === "restore" ? 1 : stage === "running" ? 2 : stage === "registry" ? 3 : 4} DE 4
                  </span>
                  <h3 id="un-modal-title">{modalTitle}</h3>
                </div>
              </div>
              <span className="un-modal-stage">{stageLabel(stage)}</span>
            </header>

            {stageError && <p className="un-alert" role="alert">{stageError}</p>}

            {stage === "restore" && (
              <div className="un-modal-body">
                <p>
                  O Pulse vai executar o desinstalador padrão do fabricante
                  {app.isMsi ? " (Windows Installer)" : ""}. Nada é apagado nesta etapa.
                </p>
                <label className="un-check">
                  <input
                    type="checkbox"
                    checked={restoreRequested}
                    disabled={!canRestore}
                    onChange={event => setRestoreRequested(event.target.checked)}
                  />
                  <span>
                    <strong>Gravar ponto de restauração antes de desinstalar</strong>
                    <small>
                      {canRestore
                        ? capabilities?.elevated
                          ? "Um ponto será criado e confirmado no log do System Restore."
                          : "O Pulse será elevado para esta etapa."
                        : (restoreBlockedReason ?? `Ponto de restauração ${UNAVAILABLE.toLowerCase()}`)}
                    </small>
                  </span>
                </label>
                {restoreBlockedReason && (
                  <p className="un-hint">
                    <ShieldAlert size={14} aria-hidden="true" /> {restoreBlockedReason}
                  </p>
                )}
                <div className="un-modal-actions">
                  <button type="button" className="un-btn-primary" onClick={() => void runUninstaller()}>
                    SEGUIR PARA O DESINSTALADOR
                  </button>
                  <button type="button" className="un-btn" onClick={() => void closeModal()}>
                    Cancelar
                  </button>
                </div>
              </div>
            )}

            {stage === "running" && (
              <div className="un-modal-body" aria-live="polite">
                <div className="un-spinner" aria-hidden="true" />
                <p role="status">
                  {restoreRequested
                    ? "Gravando o ponto de restauração e abrindo o desinstalador…"
                    : "Abrindo o desinstalador do fabricante…"}
                </p>
                <p className="un-hint">
                  O Pulse só avança quando o Windows confirmar que o programa saiu do registro.
                </p>
              </div>
            )}

            {stage === "registry" && (
              <div className="un-modal-body">
                {runningMessages.length > 0 && (
                  <ul className="un-messages">
                    {runningMessages.map(message => <li key={message}>{message}</li>)}
                  </ul>
                )}
                {outcome && !outcome.entryRemoved && (
                  <p className="un-alert" role="alert">
                    {outcome.detectionNote ?? "O Windows ainda lista este programa."}
                  </p>
                )}
                {outcome?.restorePoint && (
                  <p className="un-hint">
                    {outcome.restorePoint.created
                      ? outcome.restorePoint.verified
                        ? `Ponto de restauração confirmado: ${outcome.restorePoint.description}`
                        : `Ponto de restauração criado, sem confirmação no log: ${outcome.restorePoint.description}`
                      : (outcome.restorePoint.unavailableReason ?? `Ponto de restauração ${UNAVAILABLE.toLowerCase()}`)}
                  </p>
                )}
                <h4>Chaves de registro do aplicativo</h4>
                {registryCandidates.length === 0 ? (
                  <p className="un-hint">Nenhuma chave de registro sobrou para este programa.</p>
                ) : (
                  <>
                    <div className="un-bulk">
                      <button type="button" className="un-btn-ghost" onClick={() => setSelectedRegistry(new Set(registryCandidates.map(key => key.id)))}>
                        MARCAR TODAS
                      </button>
                      <button type="button" className="un-btn-ghost" onClick={() => setSelectedRegistry(new Set())}>
                        DESMARCAR TODAS
                      </button>
                    </div>
                    <ul className="un-keys" role="list">
                      {registryCandidates.map(key => (
                        <li key={key.id}>
                          <label className="un-check">
                            <input
                              type="checkbox"
                              checked={selectedRegistry.has(key.id)}
                              onChange={() =>
                                setSelectedRegistry(current => {
                                  const next = new Set(current);
                                  next.has(key.id) ? next.delete(key.id) : next.add(key.id);
                                  return next;
                                })
                              }
                            />
                            <span>
                              <strong>{key.path}</strong>
                              <small>
                                {key.reason} · confiança {key.confidence === "high" ? "alta" : "média"}
                              </small>
                            </span>
                          </label>
                        </li>
                      ))}
                    </ul>
                  </>
                )}
                <div className="un-modal-actions">
                  <button
                    type="button"
                    className="un-btn-primary"
                    disabled={cleaning || selectedRegistry.size === 0}
                    onClick={() => void cleanRegistry()}
                  >
                    {cleaning ? "REMOVENDO…" : `REMOVER ${selectedRegistry.size} CHAVE(S)`}
                  </button>
                  <button type="button" className="un-btn" onClick={() => setStage("folders")}>
                    IR PARA AS PASTAS
                  </button>
                </div>
                {report && <CleanupSummary report={report} />}
              </div>
            )}

            {stage === "folders" && (
              <div className="un-modal-body">
                <div className="un-selection-summary">
                  <strong>{formatBytes(selectedFolderBytes)}</strong>
                  <small>{selectedFolders.size} itens marcados</small>
                </div>
                <div className="un-bulk">
                  <button
                    type="button"
                    className="un-btn-ghost"
                    onClick={() => setSelectedFolders(new Set(collectAllFolderPaths(folderCandidates)))}
                  >
                    MARCAR TODOS
                  </button>
                  <button type="button" className="un-btn-ghost" onClick={() => setSelectedFolders(new Set())}>
                    DESMARCAR TODOS
                  </button>
                </div>
                {folderCandidates.length === 0 ? (
                  <p className="un-hint">Nenhuma pasta sobrou para este programa.</p>
                ) : (
                  <ul className="un-tree" role="tree">
                    {folderCandidates.map(folder => renderTree(folder, 0))}
                  </ul>
                )}
                <div className="un-modal-actions">
                  <button
                    type="button"
                    className="un-btn-primary"
                    disabled={cleaning || selectedFolders.size === 0}
                    onClick={() => void cleanFolders()}
                  >
                    {cleaning ? "REMOVENDO…" : "REMOVER PASTAS MARCADAS"}
                  </button>
                  <button type="button" className="un-btn" onClick={() => setStage("done")}>
                    CONCLUIR SEM REMOVER
                  </button>
                </div>
                {report && <CleanupSummary report={report} />}
              </div>
            )}

            {stage === "done" && (
              <div className="un-modal-body">
                <p>
                  <Undo2 size={15} aria-hidden="true" /> A desinstalação foi concluída.
                  {app.isMsi ? " Produtos MSI deixam alguns registros de componente no Windows Installer." : ""}
                </p>
                {report && <CleanupSummary report={report} />}
                <div className="un-modal-actions">
                  <button type="button" className="un-btn-primary" onClick={() => void closeModal()}>
                    FECHAR
                  </button>
                  <button type="button" className="un-btn" onClick={() => void loadApps(true)}>
                    ATUALIZAR LISTA
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

export function stageLabel(stage: Stage): string {
  switch (stage) {
    case "restore":
      return "Restauração";
    case "running":
      return "Desinstalando";
    case "registry":
      return "Registro";
    case "folders":
      return "Pastas";
    default:
      return "Concluído";
  }
}

export function CleanupSummary({ report }: { report: CleanupReport }) {
  return (
    <div className="un-summary" role="status">
      <strong>
        {report.registryRemoved + report.folderRemoved} itens removidos · {formatBytes(report.freedBytes)}
      </strong>
      <ul>
        {report.items.map(item => (
          <li key={item.id} className={item.removed ? "ok" : "fail"}>
            {item.removed ? "✓" : "✕"} {item.path} — {item.message}
          </li>
        ))}
      </ul>
    </div>
  );
}