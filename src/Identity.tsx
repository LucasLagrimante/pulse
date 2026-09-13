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
  "M8 6l-5 6 5 6 M16 6l5 6-5 6",
];
export function ModuleIcon({ index }: { index: number }) {
  return <svg className="module-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[index] || paths[0]} /></svg>;
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
