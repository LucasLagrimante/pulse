import { act, fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PulseLoading } from "./Identity";
import { PrivilegeIndicator, resetPrivilegeCache } from "./PrivilegeIndicator";
import {
  guessOsFromUserAgent,
  loadPlatformInfo,
  parsePlatformInfo,
  platformCopy,
  setPlatformForTests,
  usePlatform,
} from "./platform";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

function PlatformProbe() {
  const info = usePlatform();
  return <p data-testid="probe">{`${info.os}|${info.osName}|${info.desktop ?? "-"}|${info.sessionType ?? "-"}`}</p>;
}

describe("platform", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    resetPrivilegeCache();
  });

  afterEach(() => {
    setPlatformForTests(null);
  });

  it("valida a resposta do backend e descarta formatos desconhecidos", () => {
    expect(parsePlatformInfo({ os: "linux", osName: "Linux Mint 22.3", desktop: "KDE", sessionType: "x11" })).toMatchObject({
      os: "linux",
      osName: "Linux Mint 22.3",
      desktop: "KDE",
      sessionType: "x11",
      packageFormat: null,
    });
    expect(parsePlatformInfo({ os: "linux", osName: "" })?.osName).toBe("Linux");
    expect(parsePlatformInfo({ os: "haiku" })).toBeNull();
    expect(parsePlatformInfo(null)).toBeNull();
    expect(parsePlatformInfo({ elevated: true })).toBeNull();
  });

  it("palpita o SO pelo user agent e assume Windows sem pista clara", () => {
    expect(guessOsFromUserAgent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15")).toBe("linux");
    expect(guessOsFromUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("macos");
    expect(guessOsFromUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/120")).toBe("windows");
    expect(guessOsFromUserAgent("Mozilla/5.0 (linux) AppleWebKit/537.36 jsdom/26.1.0")).toBe("windows");
    expect(guessOsFromUserAgent("")).toBe("windows");
  });

  it("mantém os textos históricos do Windows", () => {
    const pc = platformCopy("windows");
    expect(pc.brandSubtitle).toBe("Windows observability");
    expect(pc.startWithOs).toBe("INICIAR COM WINDOWS");
    expect(pc.systemDisk).toBe("Disco C:");
    expect(pc.startupDetected).toBe("Itens detectados no Windows");
    expect(pc.standardPermission).toBe("PADRÃO");
    expect(pc.canRelaunchElevated).toBe(true);
    expect(pc.activityGpuSortHint).toBe("GPU agregada do grupo, somando as engines reportadas pelo Windows.");
    expect(pc.uninstallSortHint).toBe("Ordem alfabética do nome exibido pelo Windows.");
    expect(pc.cleanupSystemGroupHint).toBe("Recriados pelo Windows e pelos aplicativos");
    expect(pc.networkPathUnavailable).toContain("processo protegido pelo Windows");
    expect(pc.networkDisclaimer).toContain("Microsoft Defender");
  });

  it("não cita o Windows nos textos de Linux e macOS", () => {
    for (const os of ["linux", "macos"] as const) {
      const pc = platformCopy(os);
      for (const [key, value] of Object.entries(pc)) {
        if (typeof value === "string") expect(value, `${os}.${key}`).not.toMatch(/Windows|winget|UAC|C:|Defender|PPL/);
      }
      expect(pc.canRelaunchElevated).toBe(false);
    }
  });

  it("carrega a plataforma do backend uma vez e atualiza quem usa o hook", async () => {
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_platform_info"
        ? Promise.resolve({ os: "linux", osName: "Linux Mint 22.3", desktop: "KDE", sessionType: "x11" })
        : Promise.resolve(undefined),
    );
    setPlatformForTests(null);
    render(<PlatformProbe />);
    await act(() => loadPlatformInfo());
    expect(screen.getByTestId("probe").textContent).toBe("linux|Linux Mint 22.3|KDE|x11");
    await act(() => loadPlatformInfo());
    expect(vi.mocked(invoke).mock.calls.filter(call => call[0] === "get_platform_info")).toHaveLength(1);
  });

  it("usa a legenda de Linux na tela de carregamento", () => {
    setPlatformForTests("linux");
    render(<PulseLoading />);
    expect(screen.getByText("SEU LINUX, EM PERSPECTIVA.")).toBeTruthy();
  });

  it("no Linux não oferece reiniciar como administrador", async () => {
    setPlatformForTests("linux");
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_privilege_status" ? Promise.resolve({ elevated: false, limitations: [] }) : Promise.resolve(undefined),
    );
    render(<PrivilegeIndicator />);

    fireEvent.click(await screen.findByRole("button", { name: /permissão do pulse: usuário/i }));
    expect(screen.getByText("Pulse em modo usuário")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "REINICIAR COMO ADMINISTRADOR" })).toBeNull();
    expect(screen.queryByText(/UAC/)).toBeNull();
  });
});
