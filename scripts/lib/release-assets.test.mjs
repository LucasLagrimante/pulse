import { describe, expect, it } from "vitest";
import {
  buildReleaseManifest,
  formatSizeMb,
  matchReleaseAssets,
  restoreAssetFromEntry,
  verifyManifestHashes,
} from "./release-assets.mjs";

const SHA_A = "a".repeat(64);
const SHA_B = "b".repeat(64);
const SHA_C = "c".repeat(64);
const SHA_D = "d".repeat(64);

describe("release-assets", () => {
  it("formata tamanho em MB no padrão pt-BR", () => {
    expect(formatSizeMb(3931540)).toBe("3,7 MB");
    expect(formatSizeMb(15 * 1024 * 1024)).toBe("15,0 MB");
  });

  it("identifica assets de Windows, Linux e macOS da versão correta", () => {
    const matched = matchReleaseAssets(
      [
        "Pulse_1.4.0_x64-setup.exe",
        "Pulse_1.4.0_amd64.deb",
        "Pulse_1.4.0_amd64.AppImage",
        "Pulse_1.4.0_aarch64.dmg",
        "SHA256SUMS-linux.txt",
        "Pulse_1.3.0_x64-setup.exe",
      ],
      "1.4.0",
    );
    expect(matched).toEqual({
      windows: "Pulse_1.4.0_x64-setup.exe",
      linuxDeb: "Pulse_1.4.0_amd64.deb",
      linuxAppImage: "Pulse_1.4.0_amd64.AppImage",
      macosDmg: "Pulse_1.4.0_aarch64.dmg",
    });
  });

  it("monta release.json compatível com auto-update sem inventar plataformas ausentes", () => {
    const winOnly = buildReleaseManifest({
      version: "1.3.0",
      updatedAt: "2026-11-20",
      windows: { assetName: "Pulse_1.3.0_x64-setup.exe", sizeBytes: 3931540, sha256: SHA_A },
    });
    expect(winOnly.installer.assetUrl).toBe(
      "https://github.com/LucasLagrimante/pulse/releases/download/v1.3.0/Pulse_1.3.0_x64-setup.exe",
    );
    expect(winOnly.linux).toBeUndefined();
    expect(winOnly.macos).toBeUndefined();

    const multi = buildReleaseManifest({
      version: "1.4.0",
      updatedAt: "2026-11-20",
      windows: { assetName: "Pulse_1.4.0_x64-setup.exe", sizeBytes: 4_000_000, sha256: SHA_A },
      linuxDeb: { assetName: "Pulse_1.4.0_amd64.deb", sizeBytes: 10_000_000, sha256: SHA_B },
      linuxAppImage: { assetName: "Pulse_1.4.0_amd64.AppImage", sizeBytes: 18_000_000, sha256: SHA_C },
      macosDmg: { assetName: "Pulse_1.4.0_aarch64.dmg", sizeBytes: 9_500_000, sha256: SHA_D },
    });
    expect(multi.linux.deb.url).toBe("https://pulse.lagrimante.com.br/download/Pulse-latest-amd64.deb");
    expect(multi.linux.appimage.assetUrl).toBe(
      "https://github.com/LucasLagrimante/pulse/releases/download/v1.4.0/Pulse_1.4.0_amd64.AppImage",
    );
    expect(multi.macos.experimental).toBe(true);
    expect(multi.macos.dmg.sha256).toBe(SHA_D);
  });

  it("restaura asset a partir de entrada prévia válida e ignora inválidas", () => {
    const manifest = buildReleaseManifest({
      version: "1.4.0",
      updatedAt: "2026-11-20",
      windows: { assetName: "Pulse_1.4.0_x64-setup.exe", sizeBytes: 4_000_000, sha256: SHA_A },
      linuxDeb: { assetName: "Pulse_1.4.0_amd64.deb", sizeBytes: 10_000_000, sha256: SHA_B },
    });
    expect(restoreAssetFromEntry(manifest.linux.deb)).toEqual({
      assetName: "Pulse_1.4.0_amd64.deb",
      sizeBytes: 10_000_000,
      sha256: SHA_B,
    });
    expect(restoreAssetFromEntry({ sha256: "curto", sizeBytes: 10 })).toBeNull();
  });

  it("valida todos os hashes publicados no manifesto sem depender da ordem no JSON", () => {
    const manifest = buildReleaseManifest({
      version: "1.4.0",
      updatedAt: "2026-11-20",
      windows: { assetName: "Pulse_1.4.0_x64-setup.exe", sizeBytes: 4_000_000, sha256: SHA_A },
      linuxDeb: { assetName: "Pulse_1.4.0_amd64.deb", sizeBytes: 10_000_000, sha256: SHA_B },
      linuxAppImage: { assetName: "Pulse_1.4.0_amd64.AppImage", sizeBytes: 18_000_000, sha256: SHA_C },
    });

    expect(
      verifyManifestHashes(
        manifest,
        "1.4.0",
        { windows: SHA_A, linuxDeb: SHA_B, linuxAppImage: SHA_C },
        "release.json",
      ),
    ).toEqual([]);

    const problems = verifyManifestHashes(
      manifest,
      "1.4.0",
      { windows: SHA_A, linuxDeb: SHA_D, linuxAppImage: SHA_C, macosDmg: SHA_D },
      "release.json",
    );
    expect(problems).toHaveLength(2);
    expect(problems[0]).toMatch(/Linux \(\.deb\)/);
    expect(problems[1]).toMatch(/macOS \(\.dmg\)/);
  });
});
