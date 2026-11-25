import { invoke } from "@tauri-apps/api/core";
import {
  BatteryCharging,
  BatteryLow,
  BatteryMedium,
  Bluetooth,
  Copy,
  Check,
  Gamepad2,
  Headphones,
  Keyboard,
  Mouse,
  Pencil,
  PenTool,
  RefreshCw,
  RotateCcw,
  X,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import "./PeripheralBattery.css";

export type Capability<T> = {
  value: T | null;
  unavailableReason: string | null;
};

export type PeripheralKind =
  | "mouse"
  | "keyboard"
  | "headset"
  | "controller"
  | "stylus"
  | "other";

export type PeripheralTransport =
  | "bluetooth_classic"
  | "bluetooth_le"
  | "usb_hid"
  | "xinput"
  | "sysfs"
  | "ioreg"
  | "unknown";

export type PeripheralBatteryDevice = {
  id: string;
  name: string;
  hardwareName?: string | null;
  customName?: string | null;
  kind: PeripheralKind;
  transport: PeripheralTransport;
  connected: boolean;
  charging: boolean | null;
  batteryPercent: Capability<number>;
  source: string;
};

export type PeripheralBatterySnapshot = {
  devices: PeripheralBatteryDevice[];
  unavailableReason: string | null;
  platformNote: string | null;
  collectedAtUnixMs: number;
};

const POLL_INTERVAL_MS = 30_000;

export function sanitizeBatteryPercent(value: number | null | undefined): number | null {
  if (value == null || typeof value !== "number" || !Number.isFinite(value)) return null;
  if (value < 0 || value > 100) return null;
  return Math.round(value);
}

export function batteryLevelTone(percent: number | null): "critical" | "warning" | "good" | "unknown" {
  if (percent == null) return "unknown";
  if (percent <= 20) return "critical";
  if (percent <= 40) return "warning";
  return "good";
}

function transportLabel(transport: PeripheralTransport): string {
  switch (transport) {
    case "bluetooth_classic":
      return "Bluetooth";
    case "bluetooth_le":
      return "Bluetooth LE";
    case "usb_hid":
      return "USB 2.4GHz / HID";
    case "xinput":
      return "XInput";
    case "sysfs":
      return "Kernel HID/BT";
    case "ioreg":
      return "IOKit";
    default:
      return "Sem fio";
  }
}

const PERIPHERAL_KIND_OPTIONS: { kind: PeripheralKind; label: string }[] = [
  { kind: "mouse", label: "Mouse" },
  { kind: "keyboard", label: "Teclado" },
  { kind: "headset", label: "Fone" },
  { kind: "controller", label: "Controle" },
  { kind: "stylus", label: "Caneta" },
  { kind: "other", label: "Outro" },
];

function PeripheralKindIcon({ kind, size = 16 }: { kind: PeripheralKind; size?: number }) {
  switch (kind) {
    case "mouse":
      return <Mouse size={size} aria-hidden="true" />;
    case "keyboard":
      return <Keyboard size={size} aria-hidden="true" />;
    case "headset":
      return <Headphones size={size} aria-hidden="true" />;
    case "controller":
      return <Gamepad2 size={size} aria-hidden="true" />;
    case "stylus":
      return <PenTool size={size} aria-hidden="true" />;
    default:
      return <Bluetooth size={size} aria-hidden="true" />;
  }
}

export function PeripheralBattery({ compact = false }: { compact?: boolean }) {
  const [snapshot, setSnapshot] = useState<PeripheralBatterySnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copiedDiag, setCopiedDiag] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");
  const [savingAlias, setSavingAlias] = useState(false);

  const load = useCallback(async (force = false) => {
    if (force) setRefreshing(true);
    try {
      const data = await invoke<PeripheralBatterySnapshot>(
        force ? "refresh_peripheral_batteries" : "get_peripheral_batteries",
      );
      if (data && Array.isArray(data.devices)) {
        setSnapshot(data);
        setError(null);
      }
    } catch (err) {
      setError(typeof err === "string" ? err : "Não foi possível consultar a bateria dos periféricos.");
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void load(false);
    const id = window.setInterval(() => {
      void load(false);
    }, POLL_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, [load]);

  const startRename = (device: PeripheralBatteryDevice) => {
    setEditingId(device.id);
    setDraftName(device.name);
    setError(null);
  };

  const cancelRename = () => {
    if (savingAlias) return;
    setEditingId(null);
    setDraftName("");
  };

  const saveAlias = async (deviceId: string, alias: string) => {
    setSavingAlias(true);
    try {
      const updated = await invoke<PeripheralBatterySnapshot>("set_peripheral_alias", {
        deviceId,
        alias,
      });
      if (updated && Array.isArray(updated.devices)) {
        setSnapshot(updated);
      }
      setEditingId(null);
      setDraftName("");
      setError(null);
    } catch (err) {
      setError(typeof err === "string" ? err : "Não foi possível salvar o nome personalizado.");
    } finally {
      setSavingAlias(false);
    }
  };

  const saveKind = async (deviceId: string, kind: PeripheralKind) => {
    setSavingAlias(true);
    try {
      const updated = await invoke<PeripheralBatterySnapshot>("set_peripheral_kind", {
        deviceId,
        kind,
      });
      if (updated && Array.isArray(updated.devices)) {
        setSnapshot(updated);
      }
      setError(null);
    } catch (err) {
      setError(typeof err === "string" ? err : "Não foi possível salvar o ícone do dispositivo.");
    } finally {
      setSavingAlias(false);
    }
  };

  const handleCopyDiagnostics = async () => {
    try {
      const report = await invoke<string>("export_peripheral_diagnostics");
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(report);
      }
      setCopiedDiag(true);
      window.setTimeout(() => setCopiedDiag(false), 2200);
    } catch {
      setError("Não foi possível copiar o diagnóstico de periféricos.");
    }
  };

  const devices = snapshot?.devices ?? [];
  const lowCount = devices.filter(d => {
    const pct = sanitizeBatteryPercent(d.batteryPercent?.value);
    return pct != null && pct <= 20;
  }).length;

  if (compact) {
    return (
      <div className="pb-compact">
        {loading && !snapshot ? (
          <p className="pb-compact-note" role="status">Consultando periféricos conectados…</p>
        ) : error ? (
          <p className="pb-compact-error" role="alert">{error}</p>
        ) : snapshot?.unavailableReason ? (
          <p className="pb-compact-note" role="status">{snapshot.unavailableReason}</p>
        ) : devices.length === 0 ? (
          <p className="pb-compact-note" role="status">Nenhum periférico com bateria detectado agora.</p>
        ) : (
          <ul className="pb-compact-list">
            {devices.map(device => {
              const pct = sanitizeBatteryPercent(device.batteryPercent?.value);
              const tone = batteryLevelTone(pct);
              const isEditing = editingId === device.id;
              return (
                <li
                  key={device.id}
                  className={`pb-compact-row${device.connected ? "" : " is-disconnected"}`}
                >
                  <div className="pb-compact-top">
                    <span className="pb-compact-icon">
                      <PeripheralKindIcon kind={device.kind} size={14} />
                    </span>
                    {isEditing ? (
                      <div className="pb-rename-row pb-compact-rename">
                        <input
                          type="text"
                          className="pb-rename-input"
                          aria-label={`Novo nome para ${device.name}`}
                          value={draftName}
                          maxLength={64}
                          disabled={savingAlias}
                          autoFocus
                          onChange={e => setDraftName(e.target.value)}
                          onKeyDown={e => {
                            if (e.key === "Enter") {
                              e.preventDefault();
                              void saveAlias(device.id, draftName);
                            } else if (e.key === "Escape") {
                              e.preventDefault();
                              cancelRename();
                            }
                          }}
                        />
                        <button
                          type="button"
                          className="pb-rename-icon-btn pb-rename-confirm"
                          aria-label="Salvar nome personalizado"
                          disabled={savingAlias}
                          onClick={() => void saveAlias(device.id, draftName)}
                        >
                          <Check size={12} aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          className="pb-rename-icon-btn"
                          aria-label="Cancelar renomeação"
                          disabled={savingAlias}
                          onClick={cancelRename}
                        >
                          <X size={12} aria-hidden="true" />
                        </button>
                      </div>
                    ) : (
                      <>
                        <span className="pb-compact-name" title={device.name}>
                          {device.name}
                        </span>
                        <button
                          type="button"
                          className="pb-rename-trigger"
                          aria-label={`Renomear ${device.name}`}
                          title="Renomear dispositivo"
                          onClick={() => startRename(device)}
                        >
                          <Pencil size={11} aria-hidden="true" />
                        </button>
                      </>
                    )}
                    <span className={`pb-compact-pct tone-${tone}`}>
                      {device.charging ? <BatteryCharging size={12} aria-hidden="true" /> : null}
                      {pct != null ? `${pct}%` : "Indisponível"}
                    </span>
                  </div>
                  {isEditing && (
                    <div className="pb-kind-picker" role="group" aria-label={`Ícone para ${device.name}`}>
                      {PERIPHERAL_KIND_OPTIONS.map(opt => (
                        <button
                          key={opt.kind}
                          type="button"
                          className={`pb-kind-opt${device.kind === opt.kind ? " is-active" : ""}`}
                          title={opt.label}
                          aria-label={`Ícone ${opt.label}`}
                          aria-pressed={device.kind === opt.kind}
                          disabled={savingAlias}
                          onClick={() => void saveKind(device.id, opt.kind)}
                        >
                          <PeripheralKindIcon kind={opt.kind} size={13} />
                        </button>
                      ))}
                    </div>
                  )}
                  {pct != null && (
                    <div className="pb-compact-bar" aria-hidden="true">
                      <i className={`tone-${tone}`} style={{ width: `${pct}%` }} />
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        <button
          type="button"
          className="pb-compact-refresh"
          disabled={refreshing}
          onClick={() => void load(true)}
        >
          <RefreshCw size={12} className={refreshing ? "pb-spin" : undefined} aria-hidden="true" />
          {refreshing ? "ATUALIZANDO…" : "ATUALIZAR"}
        </button>
      </div>
    );
  }

  return (
    <section className="pb-panel" aria-label="Bateria de periféricos conectados">
      <div className="pb-toolbar">
        <div className="pb-toolbar-meta">
          <span className="pb-pill">
            {devices.length} {devices.length === 1 ? "DISPOSITIVO" : "DISPOSITIVOS"}
          </span>
          {lowCount > 0 && (
            <span className="pb-pill pb-pill-warn">
              <BatteryLow size={13} aria-hidden="true" />
              {lowCount} COM BATERIA BAIXA
            </span>
          )}
          {snapshot?.collectedAtUnixMs ? (
            <span className="pb-updated">
              Atualizado às {new Date(snapshot.collectedAtUnixMs).toLocaleTimeString("pt-BR")}
            </span>
          ) : null}
        </div>
        <div className="pb-toolbar-actions">
          <button
            type="button"
            className="pb-btn pb-btn-secondary"
            onClick={() => void handleCopyDiagnostics()}
          >
            {copiedDiag ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
            {copiedDiag ? "Diagnóstico copiado" : "Copiar diagnóstico"}
          </button>
          <button
            type="button"
            className="pb-btn pb-btn-primary"
            disabled={refreshing}
            onClick={() => void load(true)}
          >
            <RefreshCw size={14} className={refreshing ? "pb-spin" : undefined} aria-hidden="true" />
            {refreshing ? "Atualizando…" : "Atualizar agora"}
          </button>
        </div>
      </div>

      {error && (
        <p className="pb-banner pb-banner-error" role="alert">
          {error}
        </p>
      )}
      {snapshot?.unavailableReason && (
        <p className="pb-banner pb-banner-warn" role="status">
          {snapshot.unavailableReason}
        </p>
      )}

      {loading && !snapshot ? (
        <div className="pb-empty" role="status">
          <strong>Lendo dispositivos Bluetooth, BLE, USB 2.4GHz (HID) e controles…</strong>
          <span>A coleta roda em segundo plano sem travar a interface.</span>
        </div>
      ) : devices.length === 0 ? (
        <div className="pb-empty" role="status">
          <strong>Nenhum periférico sem fio com leitura de bateria foi encontrado.</strong>
          <span>
            Conecte um mouse, teclado, fone Bluetooth/BLE, dongle USB 2.4GHz (Logitech, HyperX,
            Razer, SteelSeries, Corsair) ou controle (Xbox / PlayStation) e clique em{" "}
            <em>Atualizar agora</em>.
          </span>
        </div>
      ) : (
        <div className="pb-grid">
          {devices.map(device => {
            const pct = sanitizeBatteryPercent(device.batteryPercent?.value);
            const tone = batteryLevelTone(pct);
            return (
              <article
                key={device.id}
                className={`pb-card tone-${tone}${device.connected ? "" : " is-disconnected"}`}
              >
                <div className="pb-card-head">
                  <span className="pb-card-icon">
                    <PeripheralKindIcon kind={device.kind} size={18} />
                  </span>
                  <div className="pb-card-title">
                    {editingId === device.id ? (
                      <div className="pb-rename-box">
                        <div className="pb-rename-row">
                          <input
                            type="text"
                            className="pb-rename-input"
                            value={draftName}
                            maxLength={60}
                            autoFocus
                            disabled={savingAlias}
                            aria-label={`Novo nome para ${device.hardwareName ?? device.name}`}
                            placeholder={device.hardwareName ?? device.name}
                            onChange={event => setDraftName(event.target.value)}
                            onKeyDown={event => {
                              if (event.key === "Enter") {
                                event.preventDefault();
                                void saveAlias(device.id, draftName);
                              } else if (event.key === "Escape") {
                                event.preventDefault();
                                cancelRename();
                              }
                            }}
                          />
                          <button
                            type="button"
                            className="pb-rename-icon-btn pb-rename-confirm"
                            disabled={savingAlias}
                            title="Salvar nome"
                            aria-label="Salvar nome"
                            onClick={() => void saveAlias(device.id, draftName)}
                          >
                            <Check size={13} aria-hidden="true" />
                          </button>
                          <button
                            type="button"
                            className="pb-rename-icon-btn"
                            disabled={savingAlias}
                            title="Cancelar"
                            aria-label="Cancelar"
                            onClick={cancelRename}
                          >
                            <X size={13} aria-hidden="true" />
                          </button>
                        </div>
                        <div className="pb-kind-picker" role="group" aria-label={`Ícone para ${device.name}`}>
                          {PERIPHERAL_KIND_OPTIONS.map(opt => (
                            <button
                              key={opt.kind}
                              type="button"
                              className={`pb-kind-opt${device.kind === opt.kind ? " is-active" : ""}`}
                              title={opt.label}
                              aria-label={`Ícone ${opt.label}`}
                              aria-pressed={device.kind === opt.kind}
                              disabled={savingAlias}
                              onClick={() => void saveKind(device.id, opt.kind)}
                            >
                              <PeripheralKindIcon kind={opt.kind} size={13} />
                            </button>
                          ))}
                        </div>
                        {device.customName && (
                          <button
                            type="button"
                            className="pb-rename-restore"
                            disabled={savingAlias}
                            onClick={() => void saveAlias(device.id, "")}
                          >
                            <RotateCcw size={11} aria-hidden="true" />
                            Restaurar nome original
                          </button>
                        )}
                      </div>
                    ) : (
                      <>
                        <div className="pb-card-name-row">
                          <strong title={device.name}>{device.name}</strong>
                          <button
                            type="button"
                            className="pb-rename-trigger"
                            title="Renomear"
                            aria-label={`Renomear ${device.name}`}
                            onClick={() => startRename(device)}
                          >
                            <Pencil size={12} aria-hidden="true" />
                          </button>
                        </div>
                        {device.hardwareName && device.hardwareName !== device.name && (
                          <span
                            className="pb-card-hw-name"
                            title={`Nome original: ${device.hardwareName}`}
                          >
                            {device.hardwareName}
                          </span>
                        )}
                      </>
                    )}
                    <span className="pb-card-sub">
                      {transportLabel(device.transport)} · {device.connected ? "Conectado" : "Desconectado"}
                    </span>
                  </div>
                  <div className={`pb-card-pct tone-${tone}`}>
                    {device.charging ? (
                      <BatteryCharging size={15} aria-hidden="true" />
                    ) : tone === "critical" ? (
                      <BatteryLow size={15} aria-hidden="true" />
                    ) : (
                      <BatteryMedium size={15} aria-hidden="true" />
                    )}
                    <span>{pct != null ? `${pct}%` : "Indisponível"}</span>
                  </div>
                </div>

                {pct != null ? (
                  <div
                    className="pb-bar-track"
                    role="progressbar"
                    aria-label={`Bateria de ${device.name}`}
                    aria-valuenow={pct}
                    aria-valuemin={0}
                    aria-valuemax={100}
                  >
                    <div className={`pb-bar-fill tone-${tone}`} style={{ width: `${pct}%` }} />
                  </div>
                ) : (
                  <p className="pb-unavail-reason">
                    {device.batteryPercent?.unavailableReason ??
                      "O dispositivo não reportou o nível de bateria."}
                  </p>
                )}

                <div className="pb-card-foot">
                  <span>{device.source}</span>
                  {device.charging === true && <span className="pb-badge-charging">Carregando</span>}
                  {tone === "critical" && !device.charging && (
                    <span className="pb-badge-low">Bateria baixa</span>
                  )}
                </div>
              </article>
            );
          })}
        </div>
      )}

      {snapshot?.platformNote && <p className="pb-footnote">{snapshot.platformNote}</p>}
    </section>
  );
}
