import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AI_PROVIDER_CYCLE_MS, AiProviderCycleButton } from "./AiProviderCycleButton";

const activeName = () => document.querySelector(".ai-provider-cycle-names .is-active")?.textContent;

describe("AiProviderCycleButton", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("alterna só entre os provedores detectados e abre no exibido", () => {
    const onOpen = vi.fn();
    render(
      <AiProviderCycleButton
        className="x"
        available={["claude", "codex"]}
        prefix="ANALISAR COM"
        onOpen={onOpen}
      />,
    );

    const button = screen.getByRole("button", { name: /escolha entre Claude Code ou Codex CLI/ });
    expect(activeName()).toBe("CLAUDE CODE");
    expect(screen.queryByText("ANTIGRAVITY")).not.toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(AI_PROVIDER_CYCLE_MS);
    });
    expect(activeName()).toBe("CODEX CLI");

    fireEvent.click(button);
    expect(onOpen).toHaveBeenCalledWith("codex");
  });

  it("pausa a troca enquanto o mouse está sobre o botão", () => {
    render(<AiProviderCycleButton className="x" available={null} prefix="ANALISAR COM" onOpen={vi.fn()} />);
    expect(activeName()).toBe("ANTIGRAVITY");

    fireEvent.mouseEnter(screen.getByRole("button"));
    act(() => {
      vi.advanceTimersByTime(AI_PROVIDER_CYCLE_MS * 3);
    });
    expect(activeName()).toBe("ANTIGRAVITY");
  });

  it("não alterna nem mostra indicadores com um único provedor", () => {
    render(<AiProviderCycleButton className="x" available={["codex"]} prefix="ANALISAR COM" onOpen={vi.fn()} />);
    act(() => {
      vi.advanceTimersByTime(AI_PROVIDER_CYCLE_MS * 2);
    });
    expect(activeName()).toBe("CODEX CLI");
    expect(document.querySelector(".ai-provider-cycle-dots")).toBeNull();
  });
});
