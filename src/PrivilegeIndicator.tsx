import { invoke } from "@tauri-apps/api/core";
import { useEffect, useId, useRef, useState } from "react";

export type PrivilegeLimitation = { area: string; feature: string; detail: string };

export type PrivilegeStatus = { elevated: boolean; limitations: PrivilegeLimitation[] };

type PrivilegeState =
  | { kind: "loading" }
  | { kind: "ready"; status: PrivilegeStatus }
  | { kind: "unavailable"; reason: string };

function isPrivilegeStatus(value: unknown): value is PrivilegeStatus {
  const candidate = value as PrivilegeStatus | null;
  return candidate != null && typeof candidate.elevated === "boolean" && Array.isArray(candidate.limitations);
}

// A elevação não muda enquanto o processo vive: uma consulta serve o app inteiro.
let cached: Promise<PrivilegeState> | null = null;

function loadPrivilegeStatus(): Promise<PrivilegeState> {
  cached ??= invoke<unknown>("get_privilege_status")
    .then((value): PrivilegeState =>
      isPrivilegeStatus(value)
        ? { kind: "ready", status: value }
        : { kind: "unavailable", reason: "Resposta inválida do backend" })
    .catch((error: unknown): PrivilegeState => ({ kind: "unavailable", reason: String(error) }));
  return cached;
}

/** Só para testes: descarta o resultado em cache. */
export function resetPrivilegeCache(): void {
  cached = null;
}

export function usePrivilegeStatus(): PrivilegeState {
  const [state, setState] = useState<PrivilegeState>({ kind: "loading" });
  useEffect(() => {
    let alive = true;
    void loadPrivilegeStatus().then(next => {
      if (alive) setState(next);
    });
    return () => {
      alive = false;
    };
  }, []);
  return state;
}

/** `true` só quando foi provado que o Pulse NÃO está elevado. */
export function useIsStandardUser(): boolean {
  const state = usePrivilegeStatus();
  return state.kind === "ready" && !state.status.elevated;
}

export function PrivilegeIndicator() {
  const state = usePrivilegeStatus();
  const [open, setOpen] = useState(false);
  const [relaunching, setRelaunching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const relaunch = async () => {
    setRelaunching(true);
    setError(null);
    try {
      // Se o UAC for aceito, este processo é encerrado e a cópia elevada abre.
      await invoke("relaunch_as_admin");
    } catch (reason) {
      setError(String(reason));
      setRelaunching(false);
    }
  };

  const label =
    state.kind === "loading"
      ? "VERIFICANDO"
      : state.kind === "unavailable"
        ? "INDISPONÍVEL"
        : state.status.elevated
          ? "ADMINISTRADOR"
          : "PADRÃO";
  const tone = state.kind === "ready" ? (state.status.elevated ? "state-good" : "state-pending") : "";

  return (
    <div className="privilege-indicator" ref={rootRef}>
      <button
        type="button"
        className="privilege-trigger"
        aria-expanded={open}
        aria-controls={panelId}
        aria-label={`Permissão do Pulse: ${label.toLowerCase()}. Ver detalhes`}
        onClick={() => setOpen(value => !value)}
      >
        <span>PERMISSÃO</span>
        <b className={tone}>{label}</b>
      </button>
      {open && (
        <section className="privilege-panel" id={panelId} role="dialog" aria-label="Permissão do Pulse">
          {state.kind === "loading" && <p>Verificando o nível de permissão…</p>}
          {state.kind === "unavailable" && (
            <p>Não foi possível verificar a permissão: {state.reason}</p>
          )}
          {state.kind === "ready" && state.status.elevated && (
            <>
              <strong>Pulse em modo administrador</strong>
              <p>Todas as funcionalidades estão liberadas. Ações em pastas do sistema e serviços do Windows passam a valer de fato, então revise antes de confirmar.</p>
            </>
          )}
          {state.kind === "ready" && !state.status.elevated && (
            <>
              <strong>Pulse em modo padrão</strong>
              <p>O monitoramento e a maior parte das ações funcionam normalmente. Estas dependem de administrador:</p>
              <ul className="privilege-list">
                {state.status.limitations.map(item => (
                  <li key={`${item.area}-${item.feature}`}>
                    <span className="privilege-area">{item.area.toUpperCase()}</span>
                    <b>{item.feature}</b>
                    <small>{item.detail}</small>
                  </li>
                ))}
              </ul>
              <button type="button" className="privilege-relaunch" onClick={() => void relaunch()} disabled={relaunching}>
                {relaunching ? "AGUARDANDO O WINDOWS…" : "REINICIAR COMO ADMINISTRADOR"}
              </button>
              <small className="privilege-note">O Windows pede confirmação (UAC). O Pulse fecha e reabre com permissão elevada.</small>
              {error && <p className="cleanup-message error" role="alert">{error}</p>}
            </>
          )}
        </section>
      )}
    </div>
  );
}
