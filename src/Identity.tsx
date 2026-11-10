import { Cpu, MemoryStick, Server, ShieldCheck } from "lucide-react";
import type { SystemProcessKind } from "./App";

/** Pulse: a continuous signal folded into a P. Shared by shell and loading states. */
export function PulseMark({ animated = false }: { animated?: boolean }) {
  return <svg className={`pulse-mark${animated ? " pulse-mark-loading" : ""}`} viewBox="0 0 48 48" fill="none" aria-hidden="true">
    <path className="pulse-mark-rail" d="M8 34V14a6 6 0 0 1 6-6h20a6 6 0 0 1 6 6v20a6 6 0 0 1-6 6H20" />
    <path className="pulse-mark-signal" d="M4 26h10l5-12v24l7-20 4 8h14" />
  </svg>;
}

const paths = [
  "M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z",
  "M3 7h18 M3 12h12 M3 17h16",
  "M2 13h5l3-8 4 14 3-6h5",
  "M4 4h16v16H4z M4 14h16 M15 17h2",
  "M12 3l8 4v6c0 4-8 8-8 8s-8-4-8-8V7z M8 12l3 3 5-6",
  // Serviços e atualizações: engrenagem (manutenção) com traço de progresso.
  "M12 9a3 3 0 100 6 3 3 0 000-6z M12 2.5v3 M12 18.5v3 M2.5 12h3 M18.5 12h3 M5.2 5.2l2.1 2.1 M16.7 16.7l2.1 2.1 M18.8 5.2l-2.1 2.1 M7.3 16.7l-2.1 2.1",
  "M4 17V9 M10 17V5 M16 17v-4 M3 21h18 M19 3v6 M16 6h6",
  "M8 6l-5 6 5 6 M16 6l5 6-5 6",
];

const moduleIconPaths: Record<string, string> = {
  "Visão geral": paths[0],
  "Atividade": paths[1],
  "Desempenho": paths[2],
  "Armazenamento": paths[3],
  "Desinstalar programas": "M3 6h18 M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M10 11v6 M14 11v6",
  "Inicialização e segurança": paths[4],
  "Serviços e atualizações": paths[5],
  "Cotas de IA": paths[6],
  "Debug": paths[7],
};

export function ModuleIcon({ index, name }: { index?: number; name?: string }) {
  const d = (name ? moduleIconPaths[name] : undefined) ?? (index !== undefined ? paths[index] : undefined) ?? paths[0];
  return (
    <svg className="module-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={d} />
    </svg>
  );
}

export function FileIcon({ folder = false }: { folder?: boolean }) {
  return <svg className="file-symbol" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" aria-hidden="true"><path d={folder ? "M3 6h7l2 3h9v11H3z" : "M6 3h8l5 5v13H6z M14 3v6h5 M9 13h7 M9 17h5"} /></svg>;
}

export function PulseLoading({ compact = false }: { compact?: boolean }) {
  return <div className={compact ? "pulse-loading compact" : "pulse-loading"} role="status">
    <PulseMark animated />
    <div><strong>{compact ? "Lendo o sistema" : "Pulse"}</strong><p>Conectando à telemetria local…</p></div>
    {!compact && <>
      <span className="boot-caption">SEU WINDOWS, EM PERSPECTIVA.</span>
      <span className="boot-author">POR LUCAS LAGRIMANTE</span>
    </>}
  </div>;
}

// Glifo semântico para pseudo-processos do Windows; os demais usam a janela genérica
const SYSTEM_KIND_ICONS = {
  kernel: Cpu,
  memory: MemoryStick,
  virtualization: Server,
  security: ShieldCheck,
} as const;

export function ProcessFallbackIcon({ kind }: { kind?: SystemProcessKind | null }) {
  const KindIcon = kind != null ? SYSTEM_KIND_ICONS[kind] : undefined;
  if (KindIcon) {
    return <KindIcon className="process-fallback-symbol" size={16} strokeWidth={1.6} aria-hidden="true" />;
  }
  return (
    <svg
      className="process-fallback-symbol"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="3" y="3" width="18" height="18" rx="3" />
      <path d="M3 9h18 M9 21V9" />
    </svg>
  );
}
