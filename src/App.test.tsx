import { fireEvent, render, screen } from "@testing-library/react";
import { vi } from "vitest";
import App from "./App";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => new Promise(() => undefined)),
}));

describe("Pulse application shell", () => {
  it("renders the product identity, primary areas and unified dashboard", () => {
    render(<App />);

    expect(screen.getByRole("heading", { name: "Pulse" })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Visão geral",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Atividade",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Desempenho",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Armazenamento",
    );
    expect(screen.getByRole("navigation", { name: "Navegação principal" })).toHaveTextContent(
      "Inicialização e segurança",
    );

    expect(screen.getByRole("heading", { name: "Painel de telemetria local" })).toBeInTheDocument();
    expect(screen.getByText("CPU agora")).toBeInTheDocument();
    expect(screen.getByText("Memória RAM")).toBeInTheDocument();
    expect(screen.getByText("Disco C:")).toBeInTheDocument();
    expect(screen.getByText("Inicialização")).toBeInTheDocument();
    expect(screen.getByText("Inicialização e cobertura")).toBeInTheDocument();
    expect(screen.getByText("Iniciar com o Windows")).toBeInTheDocument();
  });

  it("opens the corresponding module from a telemetry card", () => {
    render(<App />);

    fireEvent.click(screen.getAllByRole("button", { name: "Abrir desempenho" })[0]);

    expect(screen.getByRole("button", { name: /Desempenho/ })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Desempenho — recursos em uso" })).toBeInTheDocument();
  });
});
