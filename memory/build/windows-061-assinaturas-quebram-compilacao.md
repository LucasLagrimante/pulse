---
name: windows-061-assinaturas-quebram-compilacao
description: oito divergências da crate windows 0.61 que quebram cargo check (consts de HKEY, phkresult cru, REG_SAM_FLAGS sem from_bits_truncate, FreeLibrary/HMODULE em Foundation, BOOL newtype com as_bool, PCSTR = *const u8, GetProcAddress devolve Option<fn>)
module: build
metadata:
  type: project
  pin: true
---

**Why:** No `windows` 0.61 (Cargo.lock fixado) o que era `fn()` virou `const`, e vários `Option<&mut T>`/`Option<*mut T>` foram afrouxados para ponteiro cru. Medido em `cargo check` real, não na doc.

**How to apply:**
- `HKEY_LOCAL_MACHINE`/`HKEY_CURRENT_USER`/`HKEY_USERS` são **const** (sem parênteses); `HKEY` mora em `System::Registry`, **não** em `Foundation`.
- `FreeLibrary` e `HMODULE` estão em `Win32::Foundation` (o módulo `LibraryLoader` não re-exporta nada).
- `BOOL` é **newtype** em `windows-result` (`BOOL(0)`, `x.as_bool()`); `x != 0` e `let mut x: BOOL = 0` não compilam.
- `PCSTR` é `*const u8`: de `CString::as_ptr()` (`*const i8`) precisa `.cast::<u8>()`, e `GetProcAddress` devolve `Option<unsafe extern "system" fn() -> isize>` — o `.0` não existe.
- `RegOpenKeyExW(..., phkresult: *mut HKEY)` — 5º argumento **cru**, não `Option<*mut>`; passar `&mut handle`.
- `REG_SAM_FLAGS` só tem `contains` + operadores de bit; construa `REG_SAM_FLAGS(a|b)`.
- `CoInitializeSecurity` exige `RPC_C_AUTHN_LEVEL(..)`/`RPC_C_IMP_LEVEL(..)`/`EOLE_AUTHENTICATION_CAPABILITIES(..)`.
- NTSTATUS (`0xC0000135`) não cabe em `i32` **e** `as i32` não é válido como padrão de `match`: extraia `const STATUS_X: i32 = 0xC0000135u32 as i32;` e case no nome.

Ver também `memory/build/comando-tauri-reexportado-nao-registra.md`.