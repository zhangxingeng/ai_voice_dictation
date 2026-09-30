# AI Voice Dictation

Local dictation for the Linux desktop. Press <kbd>Super</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>
anywhere, speak, press it again. The text lands in a small always-on-top window
where you can fix it and copy it. Everything runs on your machine.

- **Recording never waits.** Stop one burst and start the next while the first
  is still transcribing; results append in order.
- **Editable transcript.** Fix what Whisper got wrong before you copy.
- **English or Chinese**, picked in the window.
- **Level meter.** It stays empty until you speak, so a muted or wrong
  microphone is obvious straight away.
- **GPU without setup.** Whisper `large-v3-turbo` runs through whisper.cpp on
  Vulkan, which works with the graphics driver you already have (NVIDIA, AMD,
  Intel) and falls back to the CPU when there is no GPU.
- **One-click updates** from inside the app.

## Install

Download `ai-voice-dictation_<version>_amd64.deb` from
[Releases](https://github.com/zhangxingeng/ai_voice_dictation/releases) and open
it, or:

```sh
sudo apt install ./ai-voice-dictation_*_amd64.deb
```

Needs Ubuntu 24.04 or newer (or another Debian-based distro of that age).
Chinese text uses the system's CJK font; the package recommends
`fonts-noto-cjk`, which apt installs with it by default.

## Use

Launch **AI Voice Dictation** from the app grid. The first launch downloads the
speech model (574 MB) and registers the global shortcut with GNOME.

| | |
|---|---|
| <kbd>Super</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | start recording / stop and transcribe — works from any app |
| Language picker | applies to the recording in progress and every later one |
| **Copy** | copies the whole transcript |
| <kbd>Esc</kbd> | quit |

The status line shows what is happening: *Recording* with the level meter,
*Transcribing (n)* while bursts are queued, *Done* when the text is in.

To change the shortcut, edit it in *Settings → Keyboard → Custom Shortcuts*;
the app will not change it back. The global shortcut needs GNOME. On other
desktops, bind `ai_voice_dictation --toggle` to a key yourself.

**Updates:** when a newer release exists, an **Update** button appears in the
window. It downloads the new `.deb` and installs it after a password prompt;
restart the app to use it.

**Remove:** `ai_voice_dictation --unbind` removes the shortcut, then
`sudo apt remove ai-voice-dictation`. The model lives in
`~/.local/share/ai_voice_dictation/`.

## Development

```sh
sudo apt install cmake libclang-dev glslc libvulkan-dev libasound2-dev
cargo run --release
```

`glslc` and `libvulkan-dev` compile whisper.cpp's GPU shaders. `libclang-dev`
matters more than it looks: without it the build silently uses prebuilt
bindings that lack the Vulkan symbols, and fails later with a confusing error.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --release -- --ignored   # real model and live GitHub; see below
```

The ignored tests need `DICTATION_WAV=clip.wav` (16 kHz mono) and optionally
`DICTATION_LANG=zh`.

**Release:** bump `version` in `Cargo.toml`, commit, and push a `v*` tag. CI
builds the `.deb` and publishes it as a GitHub release, and installed copies
offer the update.

See [`STATE.md`](STATE.md) for the design and the measurements behind it.

## License

MIT — see [`LICENSE`](LICENSE).
