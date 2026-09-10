import { render, screen } from "@testing-library/react";
import App from "./App";

describe("Pulse application shell", () => {
  it("renders the product identity and the five primary areas", () => {
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
  });
});
