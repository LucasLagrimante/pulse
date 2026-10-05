---
name: volume-mixer-coreaudio
description: Mixer por app no tray usa Core Audio com agrupamento por exe, master opcional e polling pausado no ajuste
module: desktop
metadata:
  type: project
---
**Why:** Sessões Core Audio (`ISimpleAudioVolume`) são por stream, não por app; `IsSystemSoundsSession` retorna `HRESULT` onde `S_FALSE` também é "ok"; e `IAudioEndpointVolume` vive no submódulo `Endpoints` com feature própria (submódulos não herdam a feature do pai).
**How to apply:**
- Backend (`audio.rs`): agrupar sessões por exe lowercase (`system` p/ sons do sistema, por último); comparar `IsSystemSoundsSession() == S_OK` (nunca `.is_ok()`); master via `IAudioEndpointVolume` é `Option` (indisponível = `None`, nunca zero fictício); `Set*` recebe `std::ptr::null()` como event context; Cargo precisa de `Win32_Media_Audio_Endpoints` + `Win32_System_Threading`.
- Frontend: `VOLUME MIXER` recolhido por padrão (`mixerOpen=false`), controla a saída de `outputDefaultId`; slider com debounce 250ms + update otimista; poll 5s pausado durante ajuste (`mixerAdjusting`/busy) para não brigar com o arrasto.
- Ícones: `AudioMixerApp.icon` vem de `app_icons::icon_for_path` (caminho do `QueryFullProcessImageNameW`, cache compartilhado com a Inicialização); `None` = tile 22px com fallback de alto-falante, nunca ícone fictício.
