export type TopBarWidgetId =
  | "cpu"
  | "ram"
  | "gpu"
  | "network"
  | "peripherals"
  | "quotas"
  | "tray"
  | "volume"
  | "clock";

export const TOP_BAR_WIDGET_CATALOG: {
  id: TopBarWidgetId;
  label: string;
  description: string;
  locked?: boolean;
}[] = [
  {
    id: "cpu",
    label: "CPU",
    description: "Uso total de processador em tempo real.",
  },
  {
    id: "ram",
    label: "RAM",
    description: "Percentual de memória RAM em uso.",
  },
  {
    id: "gpu",
    label: "GPU",
    description: "Uso de GPU e temperatura quando suportado pelo driver.",
  },
  {
    id: "network",
    label: "Rede",
    description: "Taxa agregada de download e upload das interfaces ativas.",
  },
  {
    id: "peripherals",
    label: "Periféricos",
    description: "Menor bateria entre mouses, fones e controles conectados.",
  },
  {
    id: "quotas",
    label: "Cotas de IA",
    description: "Ícone exclusivo com a menor cota restante (clique abre popup dedicado).",
  },
  {
    id: "tray",
    label: "Ícones de outros apps",
    description: "Ícones da bandeja do sistema (sempre ativo; arraste para posicionar widgets antes ou depois).",
    locked: true,
  },
  {
    id: "volume",
    label: "Volume",
    description: "Clique alterna mudo; botão direito abre o Painel de Áudio; roda ajusta.",
  },
  {
    id: "clock",
    label: "Relógio",
    description: "Data e hora em pt-BR; clique abre calendário e configurações de data/hora.",
  },
];

export type BetaFeaturesConfig = {
  peripheralBattery: boolean;
  customDock: boolean;
  pulseRunner: boolean;
  pulseRunnerWinKey: boolean;
  dockAutoHide: boolean;
  dockHideNativeTaskbar: boolean;
  dockMagnification: boolean;
  dockIconSize: number;
  dockTheme: string;
  dockPinnedApps: string[];
  dockUnpinnedOrder?: string[];
  topBarWidgets: TopBarWidgetId[];
  topBarQuotaProviders?: string[];
  topBarPeripheralDevices?: string[];
};

export type DockStatus = {
  supported: boolean;
  running: boolean;
  nativeTaskbarHidden: boolean;
  watchdogActive: boolean;
  itemCount: number;
  engine: string;
  unavailableReason: string | null;
  lastError: string | null;
};

export type DockAppCandidate = {
  name: string;
  exePath: string;
  isPinned: boolean;
  isRunning: boolean;
};

export const DEFAULT_BETA_FEATURES: BetaFeaturesConfig = {
  peripheralBattery: false,
  customDock: false,
  pulseRunner: false,
  pulseRunnerWinKey: true,
  dockAutoHide: false,
  dockHideNativeTaskbar: false,
  dockMagnification: true,
  dockIconSize: 44,
  dockTheme: "light",
  dockPinnedApps: [],
  topBarWidgets: ["cpu", "ram", "gpu", "peripherals", "quotas", "tray", "volume", "clock"],
  topBarQuotaProviders: [],
  topBarPeripheralDevices: [],
};
