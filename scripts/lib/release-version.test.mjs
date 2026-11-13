import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  VERSION_FILES,
  bumpAll,
  bumpVersion,
  parseCommit,
  readVersions,
  renderReleaseNotes,
  suggestReleaseType,
} from "./release-version.mjs";

const commits = subjects => subjects.map(subject => ({ subject, body: "" }));

describe("release-version", () => {
  it("aplica SemVer zerando os níveis abaixo", () => {
    expect(bumpVersion("1.5.0", "patch")).toBe("1.5.1");
    expect(bumpVersion("1.5.3", "minor")).toBe("1.6.0");
    expect(bumpVersion("1.5.3", "major")).toBe("2.0.0");
    expect(() => bumpVersion("1.5", "patch")).toThrow(/MAJOR\.MINOR\.PATCH/);
    expect(() => bumpVersion("1.5.0", "menor")).toThrow(/tipo de release inválido/);
  });

  it("sugere o tipo pelos commits convencionais, ignorando o próprio processo de release", () => {
    expect(suggestReleaseType(commits(["fix(cotas): x", "chore(release): versão 1.5.0", "chore(landing): y"]))).toBe("patch");
    expect(suggestReleaseType(commits(["fix(cotas): x", "feat(debug): y"]))).toBe("minor");
    expect(suggestReleaseType(commits(["feat(debug)!: troca formato"]))).toBe("major");
    expect(suggestReleaseType([{ subject: "refactor(x): y", body: "BREAKING CHANGE: config antiga" }])).toBe("major");
  });

  it("interpreta escopo, quebra e commits fora do padrão", () => {
    expect(parseCommit({ subject: "feat(debug)!: x" })).toMatchObject({ type: "feat", scope: "debug", breaking: true, description: "x" });
    expect(parseCommit({ subject: "Merge branch main" })).toMatchObject({ type: null, breaking: false });
  });

  it("gera notas só com o que o usuário percebe", () => {
    const notes = renderReleaseNotes("1.6.0", commits([
      "feat(debug): gravar a trilha de auditoria em arquivo local",
      "fix(telemetria): tolerar caracteres de controle no JSON do PowerShell",
      "test(cotas): ajustar import",
      "chore(release): versão 1.5.0",
    ]));
    expect(notes).toContain("## Pulse 1.6.0");
    expect(notes).toContain("### Novidades\n\n- **debug**: Gravar a trilha");
    expect(notes).toContain("### Correções\n\n- **telemetria**: Tolerar");
    expect(notes).not.toContain("ajustar import");
    expect(notes).not.toContain("versão 1.5.0");
    expect(renderReleaseNotes("1.6.1", commits(["test(x): y"]))).toContain("Manutenção interna");
  });

  it("troca a versão nos 6 arquivos reais do repositório sem tocar em mais nada", () => {
    const root = join(import.meta.dirname, "..", "..");
    const contents = Object.fromEntries(VERSION_FILES.map(file => [file, readFileSync(join(root, file), "utf8")]));
    const current = readVersions(contents)["src-tauri/tauri.conf.json"];
    const next = bumpVersion(current, "minor");
    const bumped = bumpAll(contents, current, next);
    expect(new Set(Object.values(readVersions(bumped)))).toEqual(new Set([next]));
    for (const file of VERSION_FILES) {
      const before = contents[file].split("\n");
      const after = bumped[file].split("\n");
      expect(after).toHaveLength(before.length);
      const changed = after.filter((line, index) => line !== before[index]);
      expect(changed.length).toBe(file === "package-lock.json" ? 2 : 1);
    }
  });
});
