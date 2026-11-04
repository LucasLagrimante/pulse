import { useEffect, useState } from "react";
import { AI_PROVIDERS, type SupportedAiProvider } from "./AiDiskAdvisorModal";
import { ProviderIcon } from "./ProviderIcon";

/** Tempo que cada provedor fica visível no botão antes de trocar. */
export const AI_PROVIDER_CYCLE_MS = 2400;

export type AiProviderCycleButtonProps = {
  /** Provedores com CLI detectado; vazio ou nulo exibe os três suportados. */
  available: SupportedAiProvider[] | null;
  /** Texto antes do nome do provedor, ex.: "ANALISAR COM". */
  prefix: string;
  /** Texto depois do nome, ex.: "NO TERMINAL". */
  suffix?: string;
  className: string;
  onOpen: (provider: SupportedAiProvider) => void;
};

/**
 * Botão que alterna entre os provedores de IA disponíveis, deixando claro
 * antes do clique que há escolha. Pausa no hover/foco para o usuário não
 * clicar num provedor que acabou de sumir, e abre o modal já no exibido.
 */
export function AiProviderCycleButton({
  available,
  prefix,
  suffix,
  className,
  onOpen,
}: AiProviderCycleButtonProps) {
  const providers = available?.length
    ? AI_PROVIDERS.filter(p => available.includes(p.id))
    : AI_PROVIDERS;
  const [index, setIndex] = useState(0);
  const [paused, setPaused] = useState(false);
  const current = providers[index % providers.length];

  useEffect(() => {
    if (paused || providers.length < 2) return;
    const timer = window.setInterval(() => setIndex(i => (i + 1) % providers.length), AI_PROVIDER_CYCLE_MS);
    return () => window.clearInterval(timer);
  }, [paused, providers.length]);

  const names = providers.map(p => p.name);
  const choices = names.length > 1
    ? `${names.slice(0, -1).join(", ")} ou ${names[names.length - 1]}`
    : names[0];

  return (
    <button
      type="button"
      className={`ai-provider-cycle ${className}`}
      onClick={() => onOpen(current.id)}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
      aria-label={`${prefix} ${suffix ?? ""} — escolha entre ${choices}`.replace(/\s+/g, " ")}
      title={`Abrir terminal com ${choices} para analisar as maiores pastas e liberar espaço`}
    >
      <span className="ai-provider-cycle-icon" key={`icon-${current.id}`} aria-hidden="true">
        <ProviderIcon providerId={current.id} size={13} />
      </span>
      <span aria-hidden="true">{prefix}</span>
      {/* Todos os nomes ocupam a mesma célula: a largura é a do maior e o botão não pula */}
      <span className="ai-provider-cycle-names" aria-hidden="true">
        {providers.map(p => (
          <span key={p.id} className={p.id === current.id ? "is-active" : undefined}>
            {p.name.toUpperCase()}
          </span>
        ))}
      </span>
      {suffix && <span aria-hidden="true">{suffix}</span>}
      {providers.length > 1 && (
        <span className="ai-provider-cycle-dots" aria-hidden="true">
          {providers.map(p => (
            <i key={p.id} className={p.id === current.id ? "is-active" : undefined} />
          ))}
        </span>
      )}
    </button>
  );
}
