import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ActivityProcessTable } from "./ActivityProcessTable";
import type { TopProcess } from "./App";

const processes: TopProcess[] = [
  { name: "code.exe", id: 2, memoryMb: 2048, cpuPercent: 4, gpuPercent: 9, displayName: "Visual Studio Code", processCount: 12 },
  { name: "chrome.exe", id: 1, memoryMb: 512, cpuPercent: 62, gpuPercent: null, publisher: "Google LLC" },
  { name: "pulse-windows.exe", id: 3, memoryMb: 60, cpuPercent: 0.4, gpuPercent: 0 },
];

function renderTable(onTerminate = vi.fn()) {
  render(
    <ActivityProcessTable
      processes={processes}
      limit={25}
      memoryTotalGb={16}
      onTerminate={onTerminate}
      sortBy="memory"
      sortDirection="desc"
      onSortChange={vi.fn()}
    />,
  );
  return onTerminate;
}

describe("ActivityProcessTable", () => {
  it("liga cada botão Fechar à sua própria linha", () => {
    const onTerminate = renderTable();
    const rows = document.querySelectorAll(".dt-row");
    expect(rows).toHaveLength(3);

    const chromeRow = rows[1] as HTMLElement;
    expect(chromeRow).toHaveTextContent("chrome.exe · Google LLC");
    fireEvent.click(within(chromeRow).getByRole("button", { name: "Fechar chrome.exe" }));
    expect(onTerminate).toHaveBeenCalledWith(processes[1]);
  });

  it("mostra métricas honestas: GB, processos do grupo e GPU indisponível sem zero falso", () => {
    renderTable();
    const [codeRow, chromeRow] = Array.from(document.querySelectorAll(".dt-row")) as HTMLElement[];
    expect(codeRow).toHaveTextContent("2 GB");
    expect(codeRow).toHaveTextContent("12 processos");
    expect(within(chromeRow).getByTitle(/GPU indisponível/)).toHaveTextContent("—");
  });

  it("impede o Pulse de encerrar a si mesmo", () => {
    renderTable();
    expect(screen.getByRole("button", { name: "Fechar pulse-windows.exe" })).toBeDisabled();
  });

  it("filtra por nome ou editor e oferece limpar o filtro quando nada corresponde", () => {
    renderTable();
    const search = screen.getByRole("searchbox", { name: "Filtrar aplicativos em execução" });

    fireEvent.change(search, { target: { value: "google" } });
    expect(document.querySelectorAll(".dt-row")).toHaveLength(1);

    fireEvent.change(search, { target: { value: "inexistente" } });
    expect(screen.getByText(/Nenhum aplicativo corresponde/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Limpar filtro" }));
    expect(document.querySelectorAll(".dt-row")).toHaveLength(3);
  });
});
