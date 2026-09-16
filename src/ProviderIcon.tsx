import type { JSX } from "react";

export function ProviderIcon({
  providerId,
  size = 16,
  className = "",
}: {
  providerId: string;
  size?: number;
  className?: string;
}): JSX.Element {
  const normalized = providerId.toLowerCase();

  if (normalized === "claude") {
    return (
      <svg
        viewBox="0 0 24 24"
        width={size}
        height={size}
        fill="currentColor"
        className={`provider-icon provider-icon-claude ${className}`}
        aria-hidden="true"
      >
        <path d="M13.8 2.5a1.2 1.2 0 0 0-2.4 0l-.3 3.6a.6.6 0 0 1-.5.5l-3.5-.8a1.2 1.2 0 0 0-1.2 2l2.3 2.8a.6.6 0 0 1 0 .7l-3.1 1.8a1.2 1.2 0 0 0 .6 2.3l3.6.3a.6.6 0 0 1 .5.5l-.8 3.5a1.2 1.2 0 0 0 2 1.2l2.8-2.3a.6.6 0 0 1 .7 0l1.8 3.1a1.2 1.2 0 0 0 2.3-.6l.3-3.6a.6.6 0 0 1 .5-.5l3.5.8a1.2 1.2 0 0 0 1.2-2l-2.3-2.8a.6.6 0 0 1 0-.7l3.1-1.8a1.2 1.2 0 0 0-.6-2.3l-3.6-.3a.6.6 0 0 1-.5-.5l.8-3.5a1.2 1.2 0 0 0-2-1.2l-2.8 2.3a.6.6 0 0 1-.7 0L13.8 2.5z" />
      </svg>
    );
  }

  if (normalized === "codex") {
    return (
      <svg
        viewBox="0 0 24 24"
        width={size}
        height={size}
        fill="currentColor"
        className={`provider-icon provider-icon-codex ${className}`}
        aria-hidden="true"
      >
        <path d="M22.28 9.82a5.98 5.98 0 0 0-.52-4.91 6.05 6.05 0 0 0-6.51-2.9A6.06 6.06 0 0 0 4.98 3.8a5.99 5.99 0 0 0-3.95 4.3 6.05 6.05 0 0 0 1.34 7.07 5.98 5.98 0 0 0 .52 4.91 6.05 6.05 0 0 0 6.51 2.9A6.05 6.05 0 0 0 19.02 20.2a5.98 5.98 0 0 0 3.95-4.3 6.06 6.06 0 0 0-.69-6.08zm-9.04 12.06a4.5 4.5 0 0 1-2.87-1.04l.14-.08 4.78-2.76a.78.78 0 0 0 .39-.68v-6.74l2.02 1.17a.07.07 0 0 1 .04.05v5.58a4.52 4.52 0 0 1-4.5 4.5zm-8.73-4.14a4.5 4.5 0 0 1-.55-3.01l.14.09 4.78 2.76c.24.14.39.4.39.68v2.66l-2.02 1.17a.08.08 0 0 1-.06 0 4.53 4.53 0 0 1-2.68-4.35zm-1.63-8.9a4.5 4.5 0 0 1 2.32-1.97v5.67a.78.78 0 0 0 .39.68l5.84 3.37-2.02 1.17a.08.08 0 0 1-.06 0L3.5 13.34a4.52 4.52 0 0 1-.62-4.5zm15.1 3.5-5.84-3.37 2.02-1.17a.08.08 0 0 1 .06 0l4.83 2.79a4.52 4.52 0 0 1-.7 8.16v-5.73a.78.78 0 0 0-.37-.68zm2.08-2.67a4.5 4.5 0 0 1 .55 3.01l-.14-.09-4.78-2.76a.78.78 0 0 0-.39-.68V7.2l2.02-1.17a.08.08 0 0 1 .06 0 4.53 4.53 0 0 1 2.68 4.34zm-8.82-5.46a4.5 4.5 0 0 1 2.87 1.04l-.14.08-4.78 2.76a.78.78 0 0 0-.39.68v6.74L7.8 11.51a.07.07 0 0 1-.04-.05V5.88a4.52 4.52 0 0 1 4.5-4.5zm-1.22 8.35 2.6-1.5 2.6 1.5v3l-2.6 1.5-2.6-1.5z" />
      </svg>
    );
  }

  if (normalized === "antigravity") {
    return (
      <svg
        viewBox="0 0 24 24"
        width={size}
        height={size}
        fill="currentColor"
        className={`provider-icon provider-icon-antigravity ${className}`}
        aria-hidden="true"
      >
        <path d="M12 0C12 6.627 6.627 12 0 12c6.627 0 12 5.373 12 12 0-6.627 5.373-12 12-12-6.627 0-12-5.373-12-12z" />
      </svg>
    );
  }

  if (normalized === "openrouter") {
    return (
      <svg
        viewBox="0 0 24 24"
        width={size}
        height={size}
        fill="none"
        stroke="currentColor"
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={`provider-icon provider-icon-openrouter ${className}`}
        aria-hidden="true"
      >
        <path d="M4 12h11a5 5 0 0 1 5 5v3" />
        <path d="M8 8l-4 4 4 4" />
        <path d="M20 12H9a5 5 0 0 1-5-5V4" />
        <path d="M16 16l4-4-4-4" />
      </svg>
    );
  }

  return (
    <span
      className={`provider-icon provider-icon-generic ${className}`}
      aria-hidden="true"
      style={{ fontSize: Math.max(10, Math.round(size * 0.75)), fontWeight: 700, lineHeight: 1 }}
    >
      {providerId.charAt(0).toUpperCase()}
    </span>
  );
}
