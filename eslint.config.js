// Lint do frontend: foco em bugs reais (hooks do React e TypeScript), sem regras de estilo.
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "landing", "src-tauri", "node_modules", "scripts", "docs"] },
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    plugins: { "react-hooks": reactHooks },
    rules: {
      // Só as duas regras clássicas; as regras do React Compiler (v7 "recommended") ficam de fora.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
      // `catch {}` é usado de propósito (ex.: localStorage indisponível); outros blocos vazios continuam proibidos.
      "no-empty": ["error", { allowEmptyCatch: true }],
      // `_` sinaliza parâmetro/variável intencionalmente não usado.
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
    },
  },
  {
    // Testes: mocks e fixtures usam `any` com frequência legítima.
    files: ["src/**/*.test.{ts,tsx}", "src/test/**"],
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
    rules: { "@typescript-eslint/no-explicit-any": "off" },
  },
);
