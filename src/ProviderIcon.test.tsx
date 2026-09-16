import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { ProviderIcon } from "./ProviderIcon";

describe("ProviderIcon", () => {
  it("renders Claude provider icon with proper SVG and class", () => {
    const { container } = render(<ProviderIcon providerId="claude" size={18} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("18");
    expect(svg?.classList.contains("provider-icon-claude")).toBe(true);
  });

  it("renders Codex provider icon with proper SVG and class", () => {
    const { container } = render(<ProviderIcon providerId="codex" size={16} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("16");
    expect(svg?.classList.contains("provider-icon-codex")).toBe(true);
  });

  it("renders Antigravity provider icon with proper SVG and class", () => {
    const { container } = render(<ProviderIcon providerId="antigravity" size={20} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("20");
    expect(svg?.classList.contains("provider-icon-antigravity")).toBe(true);
  });

  it("renders OpenRouter provider icon with proper SVG and class", () => {
    const { container } = render(<ProviderIcon providerId="openrouter" size={14} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("14");
    expect(svg?.classList.contains("provider-icon-openrouter")).toBe(true);
  });

  it("renders OpenCode Go provider icon with the official mark and class", () => {
    const { container } = render(<ProviderIcon providerId="opencode-go" size={15} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("15");
    expect(svg?.getAttribute("viewBox")).toBe("128 96 256 320");
    expect(svg?.classList.contains("provider-icon-opencode-go")).toBe(true);
  });

  it("renders fallback text mark for unknown providers", () => {
    const { container } = render(<ProviderIcon providerId="unknown_ai" size={16} />);
    expect(container.textContent).toBe("U");
  });
});
