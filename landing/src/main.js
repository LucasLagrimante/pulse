/**
 * Pulse LP — interações
 *
 * Escopo enxuto e zero dependências externas: alternância de tema, simulador
 * interativo dos 9 módulos + Tray Flyout & Mixer Core Audio, cópia do hash
 * SHA-256, FAQ acordeão e revelação ao rolar.
 */

const STORAGE_KEY = "pulse-theme";

function getPreferredTheme() {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "light" || saved === "dark") return saved;
  } catch {
    // localStorage pode lançar em contexto privado; o fallback é o sistema.
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(theme) {
  document.documentElement.setAttribute("data-theme", theme);
  const btn = document.querySelector("[data-theme-toggle]");
  if (btn) {
    btn.setAttribute("aria-label", theme === "dark" ? "Ativar tema claro" : "Ativar tema escuro");
    btn.setAttribute("aria-pressed", String(theme === "dark"));
  }
}

// Aplicado no <head> por um script inline para não haver piscada de tema.
applyTheme(getPreferredTheme());

document.addEventListener("click", event => {
  const toggle = event.target.closest("[data-theme-toggle]");
  if (!toggle) return;
  const next = document.documentElement.getAttribute("data-theme") === "dark" ? "light" : "dark";
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // sem persistência: a escolha vale só para esta sessão
  }
  applyTheme(next);
});

/* ---------- Simulador Interativo da Interface (Módulos + Tray Flyout) ---------- */

const surfaceButtons = document.querySelectorAll("[data-sim-surface-btn]");
const surfaceViews = document.querySelectorAll("[data-sim-surface-view]");
const simTabs = [...document.querySelectorAll("[data-sim-tab]")];
const simPanels = document.querySelectorAll("[data-sim-panel]");
const titlebarFile = document.querySelector("[data-sim-title]");

function activateSurface(surfaceId) {
  surfaceButtons.forEach(btn => {
    const active = btn.getAttribute("data-sim-surface-btn") === surfaceId;
    btn.setAttribute("aria-selected", String(active));
  });
  surfaceViews.forEach(view => {
    const active = view.getAttribute("data-sim-surface-view") === surfaceId;
    if (active) view.removeAttribute("hidden");
    else view.setAttribute("hidden", "");
  });
  if (titlebarFile) {
    if (surfaceId === "tray") {
      titlebarFile.textContent = "pulse — painel rápido (tray flyout) · mixer core audio";
    } else {
      const currentTab = simTabs.find(t => t.getAttribute("aria-selected") === "true");
      const label = currentTab?.getAttribute("data-sim-label") || "visão geral";
      titlebarFile.textContent = `pulse — ${label}`;
    }
  }
}

function activateModuleTab(moduleId, focusTab = false) {
  activateSurface("main");
  simTabs.forEach(tab => {
    const active = tab.getAttribute("data-sim-tab") === moduleId;
    tab.setAttribute("aria-selected", String(active));
    tab.setAttribute("tabindex", active ? "0" : "-1");
    if (active && focusTab) tab.focus();
    if (active && titlebarFile) {
      const label = tab.getAttribute("data-sim-label") || moduleId;
      titlebarFile.textContent = `pulse — ${label}`;
    }
  });
  simPanels.forEach(panel => {
    const active = panel.getAttribute("data-sim-panel") === moduleId;
    if (active) panel.removeAttribute("hidden");
    else panel.setAttribute("hidden", "");
  });
}

surfaceButtons.forEach(btn => {
  btn.addEventListener("click", () => {
    const target = btn.getAttribute("data-sim-surface-btn");
    if (target) activateSurface(target);
  });
});

simTabs.forEach((tab, index) => {
  tab.addEventListener("click", () => {
    const moduleId = tab.getAttribute("data-sim-tab");
    if (moduleId) activateModuleTab(moduleId);
  });

  tab.addEventListener("keydown", event => {
    let nextIndex = -1;
    if (event.key === "ArrowDown" || event.key === "ArrowRight") {
      nextIndex = (index + 1) % simTabs.length;
    } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
      nextIndex = (index - 1 + simTabs.length) % simTabs.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = simTabs.length - 1;
    }
    if (nextIndex >= 0) {
      event.preventDefault();
      const targetId = simTabs[nextIndex].getAttribute("data-sim-tab");
      if (targetId) activateModuleTab(targetId, true);
    }
  });
});

// Botões "Ver no simulador" nos cards de recursos
document.querySelectorAll("[data-open-sim]").forEach(btn => {
  btn.addEventListener("click", () => {
    const target = btn.getAttribute("data-open-sim");
    if (!target) return;
    if (target === "tray") {
      activateSurface("tray");
    } else {
      activateModuleTab(target);
    }
    const visual = document.getElementById("previa");
    visual?.scrollIntoView({ behavior: "smooth", block: "center" });
  });
});

// Sliders interativos do Mixer de Volume no Tray Flyout
document.querySelectorAll("[data-mixer-slider]").forEach(slider => {
  slider.addEventListener("input", () => {
    const row = slider.closest(".tray-mixer-item");
    const output = row?.querySelector("[data-mixer-val]");
    if (output) output.textContent = `${slider.value}%`;
  });
});

/* ---------- Cópia do Checksum SHA-256 ---------- */

const copyBtn = document.querySelector("[data-copy-sha256]");
if (copyBtn) {
  copyBtn.addEventListener("click", async () => {
    const hashEl = document.querySelector("[data-release-sha256]");
    const hash = hashEl?.textContent?.trim();
    if (!hash) return;
    try {
      await navigator.clipboard.writeText(hash);
      const original = copyBtn.textContent;
      copyBtn.textContent = "Copiado ✓";
      setTimeout(() => {
        copyBtn.textContent = original;
      }, 2000);
    } catch {
      // sem clipboard API disponível
    }
  });
}

/* ---------- FAQ: acordeão com um item aberto por vez ---------- */

document.querySelectorAll("[data-faq-item]").forEach(item => {
  const question = item.querySelector("[data-faq-q]");
  const answer = item.querySelector("[data-faq-a]");
  if (!question || !answer) return;

  question.addEventListener("click", () => {
    const isOpen = item.getAttribute("data-open") === "true";
    document.querySelectorAll("[data-faq-item]").forEach(other => {
      other.setAttribute("data-open", "false");
      other.querySelector("[data-faq-q]")?.setAttribute("aria-expanded", "false");
    });
    item.setAttribute("data-open", String(!isOpen));
    question.setAttribute("aria-expanded", String(!isOpen));
  });
});

/* ---------- Revelação ao rolar ---------- */

const revealTargets = document.querySelectorAll("[data-reveal]");

if (revealTargets.length) {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reduced || !("IntersectionObserver" in window)) {
    revealTargets.forEach(el => el.classList.add("is-visible"));
  } else {
    const observer = new IntersectionObserver(entries => {
      entries.forEach(entry => {
        if (!entry.isIntersecting) return;
        entry.target.classList.add("is-visible");
        observer.unobserve(entry.target);
      });
    }, { rootMargin: "0px 0px -12% 0px", threshold: 0.06 });

    revealTargets.forEach(el => observer.observe(el));
  }
}

window.addEventListener("load", () => {
  requestAnimationFrame(() => {
    revealTargets.forEach(el => {
      const rect = el.getBoundingClientRect();
      if (rect.top < window.innerHeight && rect.bottom > -window.innerHeight) {
        el.classList.add("is-visible");
      }
    });
  });
});

/* ---------- Cabeçalho com fundo ao rolar ---------- */

const header = document.querySelector("[data-header]");
if (header) {
  const onScroll = () => header.classList.toggle("is-scrolled", window.scrollY > 12);
  onScroll();
  window.addEventListener("scroll", onScroll, { passive: true });
}

/* ---------- Ano no rodapé ---------- */

const year = document.querySelector("[data-year]");
if (year) year.textContent = String(new Date().getFullYear());
