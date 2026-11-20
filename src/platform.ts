import { invoke } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";

/**
 * Plataforma em que o Pulse está rodando. Consultada uma vez no backend
 * (`get_platform_info`) e compartilhada pelo app inteiro: troca os textos que
 * citam o Windows e esconde ações que só existem lá.
 */
export type PlatformOs = "windows" | "linux" | "macos";

export type PlatformInfo = {
  os: PlatformOs;
  osName: string;
  version: string | null;
  desktop: string | null;
  sessionType: string | null;
  packageFormat: string | null;
  arch: string | null;
};

const KNOWN_OS: readonly PlatformOs[] = ["windows", "linux", "macos"];

function isKnownOs(value: unknown): value is PlatformOs {
  return typeof value === "string" && (KNOWN_OS as readonly string[]).includes(value);
}

const textOrNull = (value: unknown): string | null =>
  typeof value === "string" && value.trim() !== "" ? value : null;

/** Valida a resposta do backend; qualquer formato inesperado é descartado. */
export function parsePlatformInfo(value: unknown): PlatformInfo | null {
  const candidate = value as Record<string, unknown> | null;
  if (candidate == null || typeof candidate !== "object" || !isKnownOs(candidate.os)) return null;
  return {
    os: candidate.os,
    osName: textOrNull(candidate.osName) ?? OS_LABEL[candidate.os],
    version: textOrNull(candidate.version),
    desktop: textOrNull(candidate.desktop),
    sessionType: textOrNull(candidate.sessionType),
    packageFormat: textOrNull(candidate.packageFormat),
    arch: textOrNull(candidate.arch),
  };
}

const OS_LABEL: Record<PlatformOs, string> = { windows: "Windows", linux: "Linux", macos: "macOS" };

/**
 * Palpite síncrono para o primeiro render, antes da resposta do backend:
 * evita piscar textos de Windows no Linux. Sem pista clara (jsdom, testes),
 * assume Windows, que é o comportamento histórico do app.
 */
export function guessOsFromUserAgent(userAgent: string): PlatformOs {
  if (/jsdom/i.test(userAgent)) return "windows";
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "macos";
  if (/X11|Linux/i.test(userAgent)) return "linux";
  return "windows";
}

function fallbackInfo(os: PlatformOs): PlatformInfo {
  return { os, osName: OS_LABEL[os], version: null, desktop: null, sessionType: null, packageFormat: null, arch: null };
}

const initialGuess = (): PlatformInfo =>
  fallbackInfo(guessOsFromUserAgent(typeof navigator === "undefined" ? "" : navigator.userAgent));

let current: PlatformInfo = initialGuess();
let loading: Promise<void> | null = null;
const listeners = new Set<() => void>();

function emit(next: PlatformInfo) {
  current = next;
  listeners.forEach(listener => listener());
}

/** Dispara (uma vez) a consulta ao backend. */
export function loadPlatformInfo(): Promise<void> {
  loading ??= Promise.resolve()
    .then(() => invoke<unknown>("get_platform_info"))
    .then(value => {
      const parsed = parsePlatformInfo(value);
      if (parsed) emit(parsed);
    })
    .catch(() => {
      // Backend antigo ou indisponível: mantém o palpite inicial.
    });
  return loading;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  void loadPlatformInfo();
  return () => {
    listeners.delete(listener);
  };
}

const getSnapshot = () => current;

/** Plataforma atual, reativa. */
export function usePlatform(): PlatformInfo {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** Leitura não reativa, para mensagens montadas dentro de callbacks. */
export function currentPlatform(): PlatformInfo {
  return current;
}

/** Só para testes: força uma plataforma (ou volta ao palpite inicial com `null`). */
export function setPlatformForTests(os: PlatformOs | null): void {
  loading = os ? Promise.resolve() : null;
  emit(os ? fallbackInfo(os) : initialGuess());
}

/**
 * Textos visíveis que dependem do SO. No Windows cada valor é idêntico ao
 * texto histórico do app (os testes dependem disso).
 */
export type PlatformCopy = {
  /** "Windows", "Linux", "macOS". */
  osLabel: string;
  /** Sujeito de frases de erro: "O Windows", "O Linux", "O macOS". */
  osSubject: string;
  brandSubtitle: string;
  aboutEyebrow: string;
  aboutTagline: string;
  bootCaption: string;
  startWithOs: string;
  startWithOsAria: string;
  systemDisk: string;
  systemDiskUpper: string;
  startupDetected: string;
  startupReading: string;
  startupIntro: string;
  startupSources: string;
  servicesIntro: string;
  aboutIntro: string;
  flyoutAudio: string;
  defaultFoldersTitle: string;
  fileManager: string;
  packageManager: string;
  packageManagerSource: string;
  packageManagerOfficial: string;
  packageManagerQuerying: string;
  servicesOutsideSystemDirs: string;
  servicesAllInSystemDirs: string;
  credentialStore: string;
  credentialSaveButton: string;
  /** Rótulo neutro do nível de permissão quando o SO não tem reinício elevado. */
  standardPermission: string;
  /** O SO oferece reiniciar o Pulse elevado (só Windows/UAC). */
  canRelaunchElevated: boolean;
  elevationConfirmOne: string;
  elevationConfirmEach: string;
  elevationHint: string;
  scanResponsiveness: string;
  adminFolderHint: string;
  uninstallSortHint: string;
  uninstallReading: string;
  uninstallWaitHint: string;
  uninstallStillListed: string;
  uninstallRegistryTitle: string;
  uninstallRegistryEmpty: string;
  privilegeElevatedDetail: string;
  folderPlaceholder: string;
  networkElevationHint: string;
  networkPathUnavailable: string;
  networkDisclaimer: string;
  activityGpuSortHint: string;
  cleanupSystemGroupHint: string;
};

export function platformCopy(os: PlatformOs): PlatformCopy {
  switch (os) {
    case "linux":
      return {
        osLabel: "Linux",
        osSubject: "O Linux",
        brandSubtitle: "Linux observability",
        aboutEyebrow: "PULSE / LINUX OBSERVABILITY",
        aboutTagline: "Uma central multiplataforma feita para observar, explicar e organizar o sistema",
        bootCaption: "SEU LINUX, EM PERSPECTIVA.",
        startWithOs: "INICIAR COM A SESSÃO",
        startWithOsAria: "Iniciar com a sessão do Linux",
        systemDisk: "Disco do sistema (/)",
        systemDiskUpper: "DISCO DO SISTEMA",
        startupDetected: "Itens detectados no Linux",
        startupReading: "Lendo os itens de inicialização do Linux…",
        startupIntro: "Inventário da inicialização da sessão e do sistema, com alterações somente onde suportadas.",
        startupSources: "Autostart XDG (~/.config/autostart e /etc/xdg/autostart) e unidades systemd do usuário e do sistema.",
        servicesIntro: "Unidades do systemd lidas direto do gerenciador do sistema e atualizações pendentes pelos gerenciadores de pacotes instalados.",
        aboutIntro: "Identidade, autoria e canais oficiais deste aplicativo local para Linux.",
        flyoutAudio: "ÁUDIO DO SISTEMA",
        defaultFoldersTitle: "Pastas padrão do perfil do usuário",
        fileManager: "Abrir no gerenciador de arquivos",
        packageManager: "gerenciador de pacotes",
        packageManagerSource: "Gerenciadores de pacotes",
        packageManagerOfficial: "repositórios do sistema",
        packageManagerQuerying: "Consultando os gerenciadores de pacotes do sistema…",
        servicesOutsideSystemDirs: "FORA DAS PASTAS DO SISTEMA",
        servicesAllInSystemDirs: "Todos os serviços ativos rodam das pastas do sistema",
        credentialStore: "chaveiro do sistema (Secret Service)",
        credentialSaveButton: "SALVAR NO CHAVEIRO",
        standardPermission: "USUÁRIO",
        canRelaunchElevated: false,
        elevationConfirmOne: "O sistema vai pedir a senha de administrador.",
        elevationConfirmEach: "O sistema pede a senha de administrador para cada um.",
        elevationHint: "o sistema pede a senha de administrador",
        scanResponsiveness: "amostra limitada para proteger a responsividade do sistema",
        adminFolderHint: "Pasta do sistema: sem permissão de administrador a análise fica parcial e itens protegidos são pulados.",
        uninstallSortHint: "Ordem alfabética do nome exibido pelo sistema.",
        uninstallReading: "Lendo a lista de programas instalados…",
        uninstallWaitHint: "O Pulse só avança quando o Linux confirmar que o programa saiu da lista de instalados.",
        uninstallStillListed: "O Linux ainda lista este programa.",
        uninstallRegistryTitle: "Entradas do sistema do aplicativo",
        uninstallRegistryEmpty: "Nenhuma entrada do sistema sobrou para este programa.",
        privilegeElevatedDetail: "Todas as funcionalidades estão liberadas. Ações em pastas e serviços do sistema passam a valer de fato, então revise antes de confirmar.",
        folderPlaceholder: "Ex: ~/Imagens/Capturas ou /mnt/dados/Projetos",
        networkElevationHint: "Sem permissão de root, conexões de outros usuários não identificam o executável",
        networkPathUnavailable: "Caminho indisponível: processo de outro usuário ou protegido pelo kernel (sem permissão de leitura em /proc).",
        networkDisclaimer: "O Pulse mostra o que foi medido direto no sistema (/proc, /sys e serviços locais). Ele não classifica programas como malware e não substitui ferramentas dedicadas de segurança.",
        activityGpuSortHint: "GPU agregada do grupo, somando os motores reportados pelo driver gráfico.",
        cleanupSystemGroupHint: "Recriados pelo sistema e pelos aplicativos",
      };
    case "macos":
      return {
        osLabel: "macOS",
        osSubject: "O macOS",
        brandSubtitle: "macOS observability",
        aboutEyebrow: "PULSE / MACOS OBSERVABILITY",
        aboutTagline: "Uma central multiplataforma feita para observar, explicar e organizar o sistema",
        bootCaption: "SEU MAC, EM PERSPECTIVA.",
        startWithOs: "INICIAR COM O MACOS",
        startWithOsAria: "Iniciar com o macOS",
        systemDisk: "Disco do sistema (/)",
        systemDiskUpper: "DISCO DO SISTEMA",
        startupDetected: "Itens detectados no macOS",
        startupReading: "Lendo os itens de inicialização do macOS…",
        startupIntro: "Inventário de itens de login e agentes do launchd, com alterações somente onde suportadas.",
        startupSources: "LaunchAgents e LaunchDaemons do usuário e do sistema.",
        servicesIntro: "Serviços do launchd lidos direto do sistema.",
        aboutIntro: "Identidade, autoria e canais oficiais deste aplicativo local para macOS.",
        flyoutAudio: "ÁUDIO DO SISTEMA",
        defaultFoldersTitle: "Pastas padrão do perfil do usuário",
        fileManager: "Mostrar no Finder",
        packageManager: "gerenciador de pacotes",
        packageManagerSource: "Gerenciadores de pacotes",
        packageManagerOfficial: "fontes do sistema",
        packageManagerQuerying: "Consultando os gerenciadores de pacotes do sistema…",
        servicesOutsideSystemDirs: "FORA DAS PASTAS DO SISTEMA",
        servicesAllInSystemDirs: "Todos os serviços ativos rodam das pastas do sistema",
        credentialStore: "Keychain do macOS",
        credentialSaveButton: "SALVAR NO KEYCHAIN",
        standardPermission: "USUÁRIO",
        canRelaunchElevated: false,
        elevationConfirmOne: "O sistema vai pedir a senha de administrador.",
        elevationConfirmEach: "O sistema pede a senha de administrador para cada um.",
        elevationHint: "o sistema pede a senha de administrador",
        scanResponsiveness: "amostra limitada para proteger a responsividade do sistema",
        adminFolderHint: "Pasta do sistema: sem permissão de administrador a análise fica parcial e itens protegidos são pulados.",
        uninstallSortHint: "Ordem alfabética do nome exibido pelo sistema.",
        uninstallReading: "Lendo a lista de programas instalados…",
        uninstallWaitHint: "O Pulse só avança quando o macOS confirmar que o programa saiu da lista de instalados.",
        uninstallStillListed: "O macOS ainda lista este programa.",
        uninstallRegistryTitle: "Entradas do sistema do aplicativo",
        uninstallRegistryEmpty: "Nenhuma entrada do sistema sobrou para este programa.",
        privilegeElevatedDetail: "Todas as funcionalidades estão liberadas. Ações em pastas e serviços do sistema passam a valer de fato, então revise antes de confirmar.",
        folderPlaceholder: "Ex: ~/Imagens/Capturas ou /mnt/dados/Projetos",
        networkElevationHint: "Sem permissão administrativa, conexões de outros usuários não identificam o executável",
        networkPathUnavailable: "Caminho indisponível: processo de outro usuário ou protegido pelo sistema.",
        networkDisclaimer: "O Pulse mostra o que foi medido por interfaces do próprio macOS. Ele não classifica programas como malware e não substitui as proteções nativas do sistema.",
        activityGpuSortHint: "GPU agregada do grupo, somando os motores reportados pelo sistema.",
        cleanupSystemGroupHint: "Recriados pelo sistema e pelos aplicativos",
      };
    default:
      return {
        osLabel: "Windows",
        osSubject: "O Windows",
        brandSubtitle: "Windows observability",
        aboutEyebrow: "PULSE / WINDOWS OBSERVABILITY",
        aboutTagline: "Uma central Windows-first feita para observar, explicar e organizar o sistema",
        bootCaption: "SEU WINDOWS, EM PERSPECTIVA.",
        startWithOs: "INICIAR COM WINDOWS",
        startWithOsAria: "Iniciar com o Windows",
        systemDisk: "Disco C:",
        systemDiskUpper: "DISCO C:",
        startupDetected: "Itens detectados no Windows",
        startupReading: "Lendo os itens de inicialização do Windows…",
        startupIntro: "Inventário do boot do Windows e alterações somente onde suportadas.",
        startupSources: "",
        servicesIntro: "Serviços do Windows lidos direto do gerenciador do sistema e atualizações pendentes pela fonte oficial do Windows.",
        aboutIntro: "Identidade, autoria e canais oficiais deste aplicativo local para Windows.",
        flyoutAudio: "ÁUDIO DO WINDOWS",
        defaultFoldersTitle: "Pastas padrão do perfil Windows",
        fileManager: "Abrir no Explorador de Arquivos do Windows",
        packageManager: "winget",
        packageManagerSource: "Repositório winget",
        packageManagerOfficial: "winget oficial",
        packageManagerQuerying: "Consultando repositório oficial do Gerenciador de Pacotes do Windows (winget)…",
        servicesOutsideSystemDirs: "FORA DAS PASTAS DO WINDOWS",
        servicesAllInSystemDirs: "Todos os serviços ativos rodam das pastas do Windows",
        credentialStore: "Credential Manager do Windows",
        credentialSaveButton: "SALVAR NO WINDOWS",
        standardPermission: "PADRÃO",
        canRelaunchElevated: true,
        elevationConfirmOne: "O Windows vai pedir confirmação.",
        elevationConfirmEach: "O Windows pede confirmação para cada um.",
        elevationHint: "o Windows pede confirmação",
        scanResponsiveness: "amostra limitada para proteger a responsividade do Windows",
        adminFolderHint: "Pasta do sistema: sem administrador a análise fica parcial e itens protegidos são pulados. Use PERMISSÃO no topo para reiniciar como administrador.",
        uninstallSortHint: "Ordem alfabética do nome exibido pelo Windows.",
        uninstallReading: "Lendo o registro de programas instalados…",
        uninstallWaitHint: "O Pulse só avança quando o Windows confirmar que o programa saiu do registro.",
        uninstallStillListed: "O Windows ainda lista este programa.",
        uninstallRegistryTitle: "Chaves de registro do aplicativo",
        uninstallRegistryEmpty: "Nenhuma chave de registro sobrou para este programa.",
        privilegeElevatedDetail: "Todas as funcionalidades estão liberadas. Ações em pastas do sistema e serviços do Windows passam a valer de fato, então revise antes de confirmar.",
        folderPlaceholder: "Ex: D:\\Images\\Screenshots ou D:\\Projetos",
        networkElevationHint: "Alguns sinais exigem privilégio administrativo",
        networkPathUnavailable: "Caminho indisponível: processo protegido pelo Windows ou Pulse sem privilégio elevado.",
        networkDisclaimer: "O Pulse mostra o que foi medido por APIs do próprio Windows. Ele não classifica programas como malware e não substitui o Microsoft Defender.",
        activityGpuSortHint: "GPU agregada do grupo, somando as engines reportadas pelo Windows.",
        cleanupSystemGroupHint: "Recriados pelo Windows e pelos aplicativos",
      };
  }
}

/** Textos por plataforma, reativos. */
export function usePlatformCopy(): PlatformCopy {
  return platformCopy(usePlatform().os);
}

/** Textos da plataforma atual, para uso fora de render (callbacks). */
export function currentPlatformCopy(): PlatformCopy {
  return platformCopy(current.os);
}
