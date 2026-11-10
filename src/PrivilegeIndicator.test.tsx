import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PrivilegeIndicator, resetPrivilegeCache } from "./PrivilegeIndicator";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const standard = {
  elevated: false,
  limitations: [
    { area: "Desinstalar", feature: "Ponto de restauração antes de desinstalar", detail: "Exige privilégio administrativo." },
  ],
};

describe("PrivilegeIndicator", () => {
  beforeEach(() => {
    resetPrivilegeCache();
    vi.mocked(invoke).mockReset();
  });

  it("shows standard mode, lists what needs admin and surfaces a cancelled UAC", async () => {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_privilege_status") return Promise.resolve(standard);
      if (command === "relaunch_as_admin") return Promise.reject("A permissão de administrador (UAC) foi cancelada.");
      return Promise.resolve(undefined);
    });
    render(<PrivilegeIndicator />);

    const trigger = await screen.findByRole("button", { name: /permissão do pulse: padrão/i });
    fireEvent.click(trigger);
    expect(screen.getByText("Ponto de restauração antes de desinstalar")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "REINICIAR COMO ADMINISTRADOR" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "A permissão de administrador (UAC) foi cancelada.");
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("relaunch_as_admin");
  });

  it("shows administrator mode without a relaunch button", async () => {
    vi.mocked(invoke).mockResolvedValue({ elevated: true, limitations: [] });
    render(<PrivilegeIndicator />);

    fireEvent.click(await screen.findByRole("button", { name: /administrador/i }));
    expect(screen.getByText("Pulse em modo administrador")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "REINICIAR COMO ADMINISTRADOR" })).toBeNull();
  });

  it("reports unavailable instead of guessing when the backend fails", async () => {
    vi.mocked(invoke).mockRejectedValue("comando desconhecido");
    render(<PrivilegeIndicator />);

    fireEvent.click(await screen.findByRole("button", { name: /indisponível/i }));
    expect(screen.getByText(/Não foi possível verificar a permissão: comando desconhecido/)).toBeTruthy();
  });
});
