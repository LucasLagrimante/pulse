import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/**
 * Estado de uma atualização disparada pelo usuário. Cada item tem o seu
 * próprio: cliques em vários pacotes correm em paralelo e cada linha mostra o
 * que está acontecendo com ela — nada de loading global que o clique seguinte
 * cancela. `message` carrega a causa já traduzida pelo backend.
 */
export type UpdateJob = {
  state: "running" | "done" | "error";
  /** Última linha emitida pelo winget para este pacote. */
  log?: string;
  message?: string;
  /** Código de saída do winget (`0x80073D28`), quando houver. */
  code?: string;
  /** A falha some ao repetir como administrador. */
  needsAdmin?: boolean;
  startedAt?: number;
  settledAt?: number;
};

/** Falha estruturada devolvida pelo backend (`UpgradeFailure`). */
export type UpgradeFailure = {
  message: string;
  code?: string | null;
  needsAdmin: boolean;
  detail?: string | null;
};

/** Pacote que não foi atualizado no lote, com a causa. */
export type BatchFailure = UpgradeFailure & { id: string; name: string };

/** Relatório de "Atualizar tudo" (`BatchUpgradeReport`). */
export type BatchUpgradeReport = {
  total: number;
  succeeded: string[];
  failed: BatchFailure[];
};

/** Evento de progresso emitido pelo backend a cada linha de saída do winget. */
type UpgradeProgress = { id: string; phase: string; line: string };

/** Id reservado do backend para a atualização em lote. */
export const ALL_PACKAGES_ID = "*";

export type UpgradeJobsState = {
  jobs: Record<string, UpdateJob>;
  busyAll: boolean;
  batchLog: string;
  batchStartedAt: number;
  /** Resultado do último lote; sobrevive à troca de área até ser lido. */
  batchNotice: string;
  /** Falha do lote inteiro (winget ausente, lista indisponível). */
  batchError: string;
  /** Pacotes que o último lote não conseguiu atualizar, com a causa. */
  batchFailures: BatchFailure[];
  batchTotal: number;
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
  batchFailures: [],
  batchTotal: 0,
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
 * Normaliza o erro do `invoke`: o backend devolve `UpgradeFailure`, mas
 * falhas da ponte (e builds antigos) devolvem string.
 */
export function toUpgradeFailure(cause: unknown): UpgradeFailure {
  if (cause && typeof cause === "object" && typeof (cause as UpgradeFailure).message === "string") {
    const failure = cause as UpgradeFailure;
    return {
      message: failure.message,
      code: failure.code ?? null,
      needsAdmin: Boolean(failure.needsAdmin),
      detail: failure.detail ?? null,
    };
  }
  return { message: String(cause), code: null, needsAdmin: false, detail: null };
}

function isBatchReport(value: unknown): value is BatchUpgradeReport {
  return (
    value != null &&
    typeof value === "object" &&
    Array.isArray((value as BatchUpgradeReport).succeeded) &&
    Array.isArray((value as BatchUpgradeReport).failed)
  );
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
      if (!job) {
        // No lote, cada pacote ganha um job quando o backend começa a
        // instalá-lo: a linha mostra o próprio progresso.
        if (state.busyAll && phase === "started") {
          return { ...prev, [id]: { state: "running", log: line, startedAt: Date.now() } };
        }
        // Evento de uma operação que a lista já não exibe: descartar.
        return prev;
      }
      if (job.state !== "running") {
        // Lote reinstalando um pacote que tinha falhado antes.
        if (state.busyAll && phase === "started") {
          return { ...prev, [id]: { state: "running", log: line, startedAt: Date.now() } };
        }
        return prev;
      }
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
  if (state.batchNotice || state.batchError || state.batchFailures.length > 0) {
    patch({ batchNotice: "", batchError: "", batchFailures: [], batchTotal: 0 });
  }
}

function failedJob(failure: UpgradeFailure, startedAt: number, settledAt: number): UpdateJob {
  return {
    state: "error",
    message: failure.message,
    log: failure.message,
    code: failure.code ?? undefined,
    needsAdmin: failure.needsAdmin,
    startedAt,
    settledAt,
  };
}

/**
 * Dispara a atualização de **um** pacote. `elevated` relança o winget como
 * administrador (o Windows mostra o aviso do UAC). A lista só é reconsultada
 * quando não houver mais nada em execução (reconsultar a cada item derruba a
 * lista e mistura estados visuais).
 */
export async function startPackageUpgrade(id: string, options: { elevated?: boolean } = {}): Promise<void> {
  if (running.has(id)) return;
  ensureProgressListener();
  running.add(id);
  patchJobs(prev => ({ ...prev, [id]: { state: "running", log: "", startedAt: Date.now() } }));
  let failure: UpgradeFailure | null = null;
  try {
    if (options.elevated) {
      await invoke<string>("upgrade_winget_package_elevated", { id });
    } else {
      await invoke<string>("upgrade_winget_package", { id });
    }
  } catch (cause) {
    failure = toUpgradeFailure(cause);
  } finally {
    running.delete(id);
  }
  const settledAt = Date.now();
  patchJobs(prev => {
    const startedAt = prev[id]?.startedAt ?? settledAt;
    const job: UpdateJob =
      failure == null
        ? { state: "done", log: "Concluído", startedAt, settledAt }
        : failedJob(failure, startedAt, settledAt);
    return { ...prev, [id]: job };
  });
  if (failure == null) {
    // O pacote sai do relatório de falhas do último lote, se estava lá.
    if (state.batchFailures.some(item => item.id === id)) {
      patch({ batchFailures: state.batchFailures.filter(item => item.id !== id) });
    }
    if (!hasRunningUpgrades()) patch({ needsRefresh: true });
  }
}

/**
 * Repete como administrador, **em sequência**, os pacotes do último lote que
 * falharam por falta de privilégio — um aviso do UAC por vez, nunca vários
 * empilhados.
 */
export async function retryBatchFailuresAsAdmin(): Promise<void> {
  const ids = state.batchFailures.filter(item => item.needsAdmin).map(item => item.id);
  for (const id of ids) {
    await startPackageUpgrade(id, { elevated: true });
  }
}

function batchNoticeFor(report: BatchUpgradeReport): string {
  if (report.total === 0) return "Nenhum aplicativo pendente para atualizar.";
  if (report.failed.length === 0) {
    return report.total === 1
      ? "O aplicativo pendente foi atualizado."
      : `Todos os ${report.total} aplicativos pendentes foram atualizados.`;
  }
  // Com falhas, o resumo vai no próprio bloco de erro; aqui só o que deu certo.
  if (report.succeeded.length === 0) return "";
  return `${report.succeeded.length} de ${report.total} aplicativos atualizados.`;
}

/** Atualiza todos os pacotes desatualizados, um por vez no backend. */
export async function startUpgradeAll(): Promise<void> {
  if (state.busyAll) return;
  ensureProgressListener();
  patch({
    busyAll: true,
    batchStartedAt: Date.now(),
    batchLog: "",
    batchNotice: "",
    batchError: "",
    batchFailures: [],
    batchTotal: 0,
  });
  try {
    const result = await invoke<BatchUpgradeReport | string>("upgrade_all_winget_packages");
    if (!isBatchReport(result)) {
      patch({
        busyAll: false,
        batchNotice: "Todos os aplicativos desatualizados foram enviados para atualização.",
        needsRefresh: true,
      });
      return;
    }
    const settledAt = Date.now();
    patchJobs(prev => {
      const next = { ...prev };
      for (const id of result.succeeded) {
        next[id] = { state: "done", log: "Concluído", startedAt: prev[id]?.startedAt ?? settledAt, settledAt };
      }
      for (const failure of result.failed) {
        next[failure.id] = failedJob(failure, prev[failure.id]?.startedAt ?? settledAt, settledAt);
      }
      return next;
    });
    patch({
      busyAll: false,
      batchNotice: batchNoticeFor(result),
      batchFailures: result.failed,
      batchTotal: result.total,
      needsRefresh: result.succeeded.length > 0,
    });
  } catch (cause) {
    patch({ busyAll: false, batchError: toUpgradeFailure(cause).message });
  }
}

/** Só para testes: volta ao estado inicial entre casos. */
export function resetUpgradeJobsForTests() {
  state = createInitialState();
  running.clear();
  progressListener = null;
  listeners.clear();
}
