// Varredura da "Remoção com rastreamento de resíduos" fora do ciclo de vida do React:
// inventário + medição de tamanhos seguem até o fim mesmo com o módulo desmontado
// (troca de módulo), e o resultado fica num cache temporário no WebView para a
// próxima abertura (inclusive do app) aparecer na hora, revalidando em segundo plano.
import { invoke } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";

import type { AppFootprint, InstalledAppsSnapshot } from "./UninstallHub";

const CACHE_KEY = "pulse.uninstall.scan.v1";
/** Cache temporário: além disso a lista é descartada em vez de exibida como "última varredura". */
const CACHE_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;
/** Reabrir o módulo depois disso relê o inventário em segundo plano (backend guarda 30 s). */
const INVENTORY_REVALIDATE_MS = 60_000;
const MEASURE_BATCH = 12;
/** Tentativas por app quando o backend adia (orçamento do lote) ou o IPC falha. */
const MAX_ATTEMPTS = 4;
const DEFERRED = "Medição adiada";

export type UninstallScanState = {
  snapshot?: InstalledAppsSnapshot;
  footprints: Map<string, AppFootprint>;
  loadState: "idle" | "loading" | "ready";
  error: string;
  measuring: boolean;
  fullMeasuringId: string | null;
  /** Momento da varredura exibida quando veio do cache em disco; `null` = já confirmada nesta sessão. */
  cachedAt: number | null;
  /** Apps já medidos (ou esgotados) nesta sessão — o restante ainda é do cache. */
  settledCount: number;
};

type Persisted = {
  v: 1;
  savedAt: number;
  snapshot: InstalledAppsSnapshot;
  footprints: AppFootprint[];
};

const initialState = (): UninstallScanState => ({
  footprints: new Map(),
  loadState: "idle",
  error: "",
  measuring: false,
  fullMeasuringId: null,
  cachedAt: null,
  settledCount: 0,
});

let state: UninstallScanState = initialState();
const listeners = new Set<() => void>();
/** Invalida trabalho assíncrono em andamento (só usado no reset). */
let generation = 0;
let pending: string[] = [];
const attempts = new Map<string, number>();
const settled = new Set<string>();
let workerActive = false;
let inventoryInflight: Promise<void> | null = null;
let inventoryFetchedAt = 0;

function setState(patch: Partial<UninstallScanState>) {
  const keys = Object.keys(patch) as (keyof UninstallScanState)[];
  if (keys.every(key => Object.is(state[key], patch[key]))) return; // sem mudança, sem re-render
  state = { ...state, ...patch };
  for (const listener of listeners) listener();
}

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function hydrate() {
  try {
    const raw = storage()?.getItem(CACHE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw) as Partial<Persisted>;
    if (
      parsed?.v !== 1 ||
      typeof parsed.savedAt !== "number" ||
      Date.now() - parsed.savedAt > CACHE_MAX_AGE_MS ||
      !Array.isArray(parsed.snapshot?.apps) ||
      !Array.isArray(parsed.footprints)
    ) {
      storage()?.removeItem(CACHE_KEY);
      return;
    }
    const footprints = new Map<string, AppFootprint>();
    for (const item of parsed.footprints) {
      if (item && typeof item.appId === "string") footprints.set(item.appId, item);
    }
    state = { ...state, snapshot: parsed.snapshot, footprints, cachedAt: parsed.savedAt };
  } catch {
    // Cache corrompido ou storage bloqueado: segue com varredura do zero.
  }
}

function persist() {
  const snapshot = state.snapshot;
  if (!snapshot || state.cachedAt != null) return; // não regrava cache antigo como se fosse novo
  try {
    const ids = new Set(snapshot.apps.map(item => item.id));
    const payload: Persisted = {
      v: 1,
      savedAt: Date.now(),
      snapshot,
      footprints: [...state.footprints.values()].filter(item => ids.has(item.appId)),
    };
    storage()?.setItem(CACHE_KEY, JSON.stringify(payload));
  } catch {
    // Cota cheia ou storage indisponível: só perde a aceleração da próxima abertura.
  }
}

function countSettled(): number {
  const apps = state.snapshot?.apps ?? [];
  return apps.reduce((total, item) => total + (settled.has(item.id) ? 1 : 0), 0);
}

/** Cache é "consolidado" quando inventário e todas as medições foram refeitos nesta sessão. */
function refreshCacheStatus() {
  const apps = state.snapshot?.apps ?? [];
  const settledCount = countSettled();
  const done = inventoryFetchedAt > 0 && settledCount >= apps.length;
  setState({ settledCount, cachedAt: done ? null : state.cachedAt });
  // Enquanto exibe cache antigo, `persist` não grava: mistura parcial não vira "última varredura".
  if (inventoryFetchedAt > 0) persist();
}

function enqueue(ids: string[]) {
  const queued = new Set(pending);
  for (const id of ids) {
    if (settled.has(id) || queued.has(id)) continue;
    pending.push(id);
    queued.add(id);
  }
  if (pending.length && !workerActive) void runWorker();
}

function settle(id: string) {
  settled.add(id);
  attempts.delete(id);
}

function retryOrGiveUp(id: string) {
  const count = (attempts.get(id) ?? 0) + 1;
  attempts.set(id, count);
  // Esgotado: mantém a medida do cache (se houver) e não insiste nesta sessão.
  if (count >= MAX_ATTEMPTS) settle(id);
  else pending.push(id);
}

async function runWorker() {
  const gen = generation;
  workerActive = true;
  setState({ measuring: true });
  try {
    while (pending.length) {
      const ids = pending.splice(0, MEASURE_BATCH);
      let batch: AppFootprint[] = [];
      try {
        const result = await invoke<AppFootprint[]>("measure_installed_app_sizes", { appIds: ids });
        batch = Array.isArray(result) ? result : [];
      } catch {
        batch = [];
      }
      if (gen !== generation) return;
      const byId = new Map(batch.filter(item => item && typeof item.appId === "string").map(item => [item.appId, item]));
      let next: Map<string, AppFootprint> | null = null;
      for (const id of ids) {
        const item = byId.get(id);
        if (!item || item.unavailableReason === DEFERRED) {
          retryOrGiveUp(id);
          continue;
        }
        // Medição completa pedida pelo usuário não é rebaixada pela rápida.
        const current = state.footprints.get(id);
        if (!(current?.fullScan && current.complete && !item.complete)) {
          next ??= new Map(state.footprints);
          next.set(id, item);
        }
        settle(id);
      }
      if (next) setState({ footprints: next });
      refreshCacheStatus();
    }
  } finally {
    if (gen === generation) {
      workerActive = false;
      setState({ measuring: false });
    }
  }
}

/** Lê o inventário; com `forceRefresh` também remede todos (o backend responde do cache de 10 min). */
export function loadInstalledApps(forceRefresh: boolean): Promise<void> {
  if (inventoryInflight && !forceRefresh) return inventoryInflight;
  const gen = generation;
  setState({ loadState: state.loadState === "ready" ? "ready" : "loading", error: "" });
  const run = (async () => {
    try {
      const next = await invoke<InstalledAppsSnapshot>("list_installed_apps", { forceRefresh });
      if (gen !== generation) return;
      inventoryFetchedAt = Date.now();
      if (forceRefresh) {
        for (const item of next?.apps ?? []) settled.delete(item.id);
      }
      setState({ snapshot: next, loadState: "ready" });
      refreshCacheStatus();
      enqueue((next?.apps ?? []).map(item => item.id));
    } catch (cause) {
      if (gen !== generation) return;
      setState({ error: String(cause), loadState: "ready" });
    }
  })().finally(() => {
    if (inventoryInflight === run) inventoryInflight = null;
  });
  inventoryInflight = run;
  return run;
}

/** Chamado ao montar o módulo: inicia a varredura uma vez e só revalida o inventário se envelheceu. */
export function ensureUninstallScan() {
  if (inventoryInflight) return;
  if (state.loadState === "idle" || Date.now() - inventoryFetchedAt > INVENTORY_REVALIDATE_MS) {
    void loadInstalledApps(false);
  }
}

/** Medição sem teto de um único app; o resultado sobrevive à troca de módulo. */
export async function measureAppFully(appId: string) {
  const gen = generation;
  setState({ fullMeasuringId: appId, error: "" });
  try {
    const result = await invoke<AppFootprint>("measure_installed_app_size_full", { appId });
    if (gen !== generation) return;
    if (result && typeof result === "object" && result.appId) {
      settle(result.appId);
      setState({ footprints: new Map(state.footprints).set(result.appId, result) });
      refreshCacheStatus();
    }
  } catch (cause) {
    if (gen === generation) setState({ error: String(cause) });
  } finally {
    if (gen === generation) setState({ fullMeasuringId: null });
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useUninstallScan(): UninstallScanState {
  return useSyncExternalStore(subscribe, () => state, () => state);
}

/** Zera o estado do módulo (testes); `hydrate` relê o cache do storage. */
export function resetUninstallScan(options: { hydrate?: boolean } = {}) {
  generation += 1;
  pending = [];
  attempts.clear();
  settled.clear();
  workerActive = false;
  inventoryInflight = null;
  inventoryFetchedAt = 0;
  state = initialState();
  if (options.hydrate) hydrate();
  for (const listener of listeners) listener();
}

hydrate();
