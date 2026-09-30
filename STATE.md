# Project state — read this first

External memory. If context was compacted, this file is the picture.

Last updated: 2026-09-30

---

## What we're building

A dictation **app** for the Linux desktop (not a service). You launch it; it
stays open. Super+Shift+D starts recording from anywhere, pressing it again
stops. The text lands in an editable, always-on-top window you fix and copy.

- **Recording is instant and never blocks** — not on the model, the GPU, or a
  previous burst.
- **Continuous bursts.** Stop one and start the next while the first decodes.
  Results append in submission order.
- **Editable transcript**, fixed in place before copying.
- **Level meter**, so a dead microphone is visible while you speak.
- **Easy install on any Linux machine**: one `.deb` from GitHub Releases, GPU
  used automatically when there is one.
- Explicitly NOT wanted: prompt library, snippets, settings sprawl, typing into
  other apps (for now), macOS/Windows.

---

## Stack

| Layer | Choice | Why |
|---|---|---|
| Language | Rust | one native binary, no runtime to ship |
| Speech | whisper.cpp via `whisper-rs`, **Vulkan** backend | GPU on any vendor with just the graphics driver; CPU fallback built in |
| Model | `large-v3-turbo` q5_0 (574 MB), downloaded on first run | as fast and accurate as q8_0 (874 MB) here |
| Audio | `cpal` | plain ALSA → PipeWire/Pulse default source |
| UI | `egui` via `eframe` (glow, **X11 only**) | least code for a status line, a meter and a text box |
| Packaging | `cargo-deb`, built by CI on a `v*` tag | double-clickable, 7 MB, deps resolved by apt |

Why Vulkan rather than CUDA: a CUDA build drags 2.5 GB of NVIDIA libraries
(cuBLAS, cuDNN…) that must ship with the app — over GitHub's 2 GB asset limit.
Vulkan needs only `libvulkan1`, which every desktop already has.

---

## Design rules

1. **Recording and transcription are separate layers joined by a queue**
   (`session.rs`). One worker decodes one burst at a time, so order is
   guaranteed by construction — nothing to reorder.
2. **Everything slow is injected.** The session takes a transcriber closure,
   the hotkey code takes a `gsettings` runner, the model installer takes a
   reader. Tests run in milliseconds with fakes; none touch the network, the
   GPU, or the user's real dconf database, except the two opt-in `#[ignore]`
   tests (real model, live GitHub API).
3. **Never write the user's keybinding after creation.** The key is written
   when our entry is created; afterwards only the command is refreshed (it
   encodes where the binary lives). Changing the key is done in GNOME Settings.
   Never overwrite another app's `customN` slot.
4. **Models are never packaged.** Download to `.part`, verify sha256, rename.
   A rename in one directory is atomic, so presence is proof of a complete
   file and nothing is re-hashed at launch.
5. **Silence never reaches Whisper** (`vad.rs`). Whisper invents text for
   silence ("you", "."). The gate is relative to the burst's own noise floor,
   so it works at any microphone gain.
6. **Updates are a button, not a repository.** At launch the app asks
   GitHub for the latest release; if it is newer, an Update button downloads
   the `.deb` and runs `pkexec apt-get install`. An apt repository on Pages
   was considered and rejected as too much machinery; AppImage (FUSE, manual
   `chmod +x`, no launcher) and Flatpak (sandbox blocks the GNOME shortcut and
   the socket; large runtime) as poor fits. Offline, nothing shows.
7. **Cost is linear in audio length.** whisper.cpp encodes fixed 30 s windows
   and seeks by timestamp; no cutting, no overlap-stitching needed.

---

## Measured findings (don't re-derive these)

RTX 3090 Ti, Ryzen 9 7950X, machine heavily loaded (load avg ~70) during the
measurements, so absolute times are pessimistic.

| Clip | Vulkan q5_0 | Vulkan q8_0 |
|---|---|---|
| 11 s real speech | 0.16 s | 0.17 s |
| 33 s technical TTS | 0.53 s | 0.53 s |
| 131 s | 1.85 s | 1.60 s |

- **Words at the 30 s boundary are kept.** A test sentence placed across the
  boundary came through intact in every run.
- **No-GPU fallback works** — with every Vulkan driver hidden, whisper.cpp runs
  on the CPU by itself. **But never call `whisper_rs::vulkan::list_devices()`**:
  with no Vulkan driver it throws a C++ exception across FFI and aborts the
  process. `engine::device()` uses ggml's device registry instead.
- **First long input on a new machine is slow once** (6 s instead of 0.5 s for
  33 s of audio): the driver compiles GPU shaders and caches them on disk.
  `Engine::load` warms up on 1 s of silence, which covers short bursts only.
- **Language is chosen, not detected** (English default, Chinese in the
  picker). Detection cost ~0.13 s a burst and misjudges short ones. Wrong
  choice is not subtle: Mandarin decoded as English comes out *translated*.
  Chinese uses the prompt `以下是普通话的句子。` to pin simplified characters;
  verified exact on synthesized Mandarin. The language is bound when a burst
  stops, and travels with it through the queue.
- Chinese glyphs come from the system font via fontconfig (Noto Sans CJK;
  the `.deb` recommends `fonts-noto-cjk`), not embedded — it is ~20 MB.
- **Build trap:** without `libclang-dev`, bindgen cannot find `stdbool.h` and
  `whisper-rs-sys` silently falls back to prebuilt bindings **without the
  Vulkan symbols**. The error surfaces later as unresolved imports.
- Hotkey round trip: `ai_voice_dictation --toggle` starts recording in ~55 ms.
- Not measured: CPU-only speed on an idle machine.

---

## GNOME / Wayland reality (GNOME 50.1, verified on this machine)

| Want | Status |
|---|---|
| Global hotkey | ✅ GNOME custom keybinding via `gsettings` → `ai_voice_dictation --toggle` → unix socket. |
| Always-on-top | ✅ **only as an X11 window.** GNOME ignores it for native Wayland windows. The app is X11-only (XWayland), and the request is re-sent on the first frame — sent at creation it arrives before the window is mapped and GNOME drops it. Sharp at fractional scaling thanks to mutter's `xwayland-native-scaling`. |
| Position the window | ❌ not for clients. |
| Type into the focused app | ❌ `/dev/uinput` is root-only; `wtype` needs a protocol GNOME lacks. Only route is the RemoteDesktop portal (consent dialog, unproven). **Clipboard only.** |
| Launcher ↔ window association | `.desktop` file name, `StartupWMClass` and the X11 `WM_CLASS` are all `ai_voice_dictation`. |

A launcher must never `Exec` a generic interpreter: GNOME's Resources groups
processes by the basename of `cmdline[0]`, so such an entry claims every
process of that interpreter on the machine, and "End" kills them all. A native
binary with its own name avoids this by construction.

---

## Architecture

```
src/
  main.rs      modes (app / --toggle / --unbind) and wiring
  control.rs   the hotkey's action: start a burst, or stop and queue it
  session.rs   burst queue, decode worker, status snapshot
  audio.rs     microphone capture, downmix, resample to 16 kHz
  engine.rs    whisper.cpp: load, warm up, transcribe, report device
  language.rs  the language choice, Chinese prompt, CJK-aware joining
  vad.rs       "is there speech in this burst?"
  meter.rs     mic level → 0..1, dB-scaled
  models.rs    first-run download, verify, atomic install
  hotkey.rs    GNOME custom keybinding registration
  ipc.rs       unix socket: one line in, one line out
  paths.rs     XDG locations
  update.rs    check GitHub for a newer release, install it via pkexec
  ui.rs        the egui window
assets/ai_voice_dictation.desktop
```

The window owns the transcript (it is editable); the session only hands it
finished fragments. The status snapshot (`session::Status`) never contains the
transcript.

---

## Open questions

1. **RemoteDesktop portal for typing into the focused app** — unproven,
   deferred.
2. **Raspberry Pi** — out of scope for this app; a separate future project.
