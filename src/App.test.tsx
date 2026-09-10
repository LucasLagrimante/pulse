import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
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
    expect(screen.queryByRole("button", { name: "Debug" })).not.toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Iniciar com o Windows" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Modo Debug" })).toBeInTheDocument();
  });

  it("opens the corresponding module from a telemetry card", () => {
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: "Abrir desempenho — CPU" }));

    expect(screen.getByRole("button", { name: /Desempenho/ })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Desempenho — recursos em uso" })).toBeInTheDocument();
  });

  it("renders desempenho after the native data arrives", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => Promise.resolve(command === "get_performance_details" ? { gpu: { value: "GPU de teste" }, vramBytes: { value: 1024 }, temperatureCelsius: { value: 42 } } : command === "get_system_overview" ? { cpuPercent: 2, memoryUsedGb: 1, memoryTotalGb: 2, diskUsedGb: 3, diskTotalGb: 4, startupItemCount: 0, topProcesses: [] } : command === "list_recent_files" ? [] : command === "list_startup_items" ? [] : { enabled: false }) as never);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Desempenho/ }));
    await waitFor(() => expect(screen.getByText("GPU de teste")).toBeInTheDocument());
  });

  it("keeps startup usable when the native command returns an invalid payload", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => Promise.resolve(command === "list_startup_items" ? null : command === "list_recent_files" ? [] : command === "get_system_overview" ? { cpuPercent: 2, memoryUsedGb: 1, memoryTotalGb: 2, diskUsedGb: 3, diskTotalGb: 4, startupItemCount: 0, topProcesses: [] } : { enabled: false }) as never);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Inicialização e segurança/ }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "Itens do Windows" })).toBeInTheDocument());
    expect(screen.getByText("Nenhum item de inicialização disponível.")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
