import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
export const ALL_PACKAGES_ID = "*";

export type UpgradeJobsState = {
  jobs: Record<string, UpdateJob>;
  busyAll: boolean;
  batchLog: string;
  batchStartedAt: number;
  /** Resultado do último lote; sobrevive à troca de área até ser lido. */
  batchNotice: string;
  batchError: string;
  /**
   * Uma instalação terminou e a lista de pendências está defasada. O hub
   * consome a flag quando está montado — inclusive ao voltar para a área
   * depois que o winget concluiu em segundo plano.
   */
  needsRefresh: boolean;
};

const createInitialState = (): UpgradeJobsState => ({
  jobs: {},
  busyAll: false,
  batchLog: "",
  batchStartedAt: 0,
  batchNotice: "",
  batchError: "",
  needsRefresh: false,
});

// Estado em escopo de módulo: o ServicesHub é desmontado ao trocar de área, mas
// o winget continua rodando no backend. Guardar aqui mantém progresso, log e
// tempo decorrido visíveis ao voltar, e impede disparar o mesmo lote de novo.
let state = createInitialState();
// Espelho síncrono dos pacotes em execução: dois cliques em sequência precisam
// ver o id já reservado, senão a mesma atualização é disparada duas vezes.
const running = new Set<string>();
let progressListener: Promise<unknown> | null = null;
const listeners = new Set<() => void>();

export function subscribeUpgradeJobs(listener: () => void): () => void {
  ensureProgressListener();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getUpgradeJobsState(): UpgradeJobsState {
  return state;
}

function patch(next: Partial<UpgradeJobsState>) {
  state = { ...state, ...next };
  for (const listener of listeners) listener();
}

function patchJobs(update: (prev: Record<string, UpdateJob>) => Record<string, UpdateJob>) {
  const jobs = update(state.jobs);
  if (jobs !== state.jobs) patch({ jobs });
}

/**
 * Listener único e permanente do progresso do backend: registrado na primeira
 * assinatura e nunca removido, para não perder linhas emitidas enquanto o hub
 * está desmontado.
 */
function ensureProgressListener() {
  if (progressListener) return;
  progressListener = listen<UpgradeProgress>("winget-upgrade-progress", event => {
    const { id, phase, line } = event.payload;
    if (id === ALL_PACKAGES_ID) {
      if (state.busyAll) patch({ batchLog: line });
      return;
    }
    patchJobs(prev => {
      const job = prev[id];
      // Evento de uma operação que a lista já não exibe: descartar.
      if (!job) return prev;
      return { ...prev, [id]: { ...job, log: phase === "finished" ? "Concluído" : line } };
    });
  }).catch(() => {
    // Sem a ponte de eventos (ex.: teste sem mock) não há progresso a mostrar;
    // permite nova tentativa na próxima assinatura.
    progressListener = null;
  });
}

export function hasRunningUpgrades(): boolean {
  return running.size > 0 || state.busyAll;
}

/** Descarta jobs concluídos de pacotes que saíram da lista de pendências. */
export function pruneUpgradeJobs(liveIds: Set<string>) {
  patchJobs(prev => {
    const next: Record<string, UpdateJob> = {};
    for (const [id, job] of Object.entries(prev)) {
      if (job.state === "running" || liveIds.has(id)) next[id] = job;
    }
    return Object.keys(next).length === Object.keys(prev).length ? prev : next;
  });
}

export function consumeRefreshRequest(): boolean {
  if (!state.needsRefresh) return false;
  patch({ needsRefresh: false });
  return true;
}

export function clearBatchMessages() {
  if (state.batchNotice || state.batchError) patch({ batchNotice: "", batchError: "" });
}

/**
 * Dispara a atualização de **um** pacote. A lista só é reconsultada quando não
 * houver mais nada em execução (reconsultar a cada item derruba a lista e
 * mistura estados visuais).
 */
export async function startPackageUpgrade(id: string): Promise<void> {
  if (running.has(id)) return;
  ensureProgressListener();
  running.add(id);
  patchJobs(prev => ({ ...prev, [id]: { state: "running", log: "", startedAt: Date.now() } }));
  let failure: string | null = null;
  try {
    await invoke<string>("upgrade_winget_package", { id });
  } catch (cause) {
    failure = String(cause);
  } finally {
    running.delete(id);
  }
  const settledAt = Date.now();
  patchJobs(prev => {
    const startedAt = prev[id]?.startedAt ?? settledAt;
    const job: UpdateJob =
      failure == null
        ? { state: "done", log: "Concluído", startedAt, settledAt }
        : { state: "error", message: failure, log: failure, startedAt, settledAt };
    return { ...prev, [id]: job };
  });
  if (failure == null && !hasRunningUpgrades()) patch({ needsRefresh: true });
}

/** Atualiza todos os pacotes desatualizados em uma única execução do winget. */
export async function startUpgradeAll(): Promise<void> {
  if (state.busyAll) return;
  ensureProgressListener();
  patch({ busyAll: true, batchStartedAt: Date.now(), batchLog: "", batchNotice: "", batchError: "" });
  try {
    await invoke<string>("upgrade_all_winget_packages");
    patch({
      busyAll: false,
      batchNotice: "Todos os aplicativos desatualizados foram enviados para atualização.",
      needsRefresh: true,
    });
  } catch (cause) {
    patch({ busyAll: false, batchError: String(cause) });
  }
}

/** Só para testes: volta ao estado inicial entre casos. */
export function resetUpgradeJobsForTests() {
  state = createInitialState();
  running.clear();
  progressListener = null;
  listeners.clear();
}
