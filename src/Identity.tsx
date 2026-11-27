import { Cpu, MemoryStick, Server, ShieldCheck } from "lucide-react";
import type { SystemProcessKind } from "./App";
import { usePlatformCopy } from "./platform";

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
  "Arquivos": "M3 6h7l2 3h9v11H3z",
  "Desinstalar programas": "M3 6h18 M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M10 11v6 M14 11v6",
  "Inicialização e segurança": paths[4],
  "Serviços e atualizações": paths[5],
  // Segurança: escudo exclusivo com cadeado interno (auditoria de acesso remoto e integridade).
  "Segurança": "M12 2.5l7.5 3.5v5.5c0 4.6-3.2 8.2-7.5 9.8-4.3-1.6-7.5-5.2-7.5-9.8V6L12 2.5z M9.5 11.5h5v4.2h-5z M10.5 11.5V10a1.5 1.5 0 0 1 3 0v1.5",
  // Rede e segurança: globo/nós de rede.
  "Rede e segurança": "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M3.5 12h17 M12 3c2.5 2.7 3.8 5.7 3.8 9s-1.3 6.3-3.8 9c-2.5-2.7-3.8-5.7-3.8-9S9.5 5.7 12 3z",
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
  return (
    <svg
      className="file-symbol"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={folder ? "M3 6h7l2 3h9v11H3z" : "M6 3h8l5 5v13H6z M14 3v6h5 M9 13h7 M9 17h5"} />
    </svg>
  );
}

/** `minimal`: só a marca animada (arranque sem splash nativa); sem textos nem legendas. */
export function PulseLoading({ compact = false, minimal = false }: { compact?: boolean; minimal?: boolean }) {
  const pc = usePlatformCopy();
  if (minimal) {
    return <div className="pulse-loading minimal" role="status" aria-label="Iniciando o Pulse"><PulseMark animated /></div>;
  }
  return <div className={compact ? "pulse-loading compact" : "pulse-loading"} role="status">
    <PulseMark animated />
    <div><strong>{compact ? "Lendo o sistema" : "Pulse"}</strong><p>Conectando à telemetria local…</p></div>
    {!compact && <>
      <span className="boot-caption">{pc.bootCaption}</span>
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
