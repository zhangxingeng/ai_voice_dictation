# AI Voice Dictation

Local dictation for the Linux desktop. Press <kbd>Super</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>
anywhere, speak, press it again. The text lands in a small always-on-top window
where you can fix it and copy it. Everything runs on your machine.

- **Recording never waits.** Stop one burst and start the next while the first
  is still transcribing; results append in order.
- **Editable transcript.** Fix what Whisper got wrong before you copy.
- **English or Chinese**, picked in the window. Pick before you stop
  recording; it applies to that burst and the ones after it.
- **Level meter**, so a muted microphone is obvious before you've said the
  whole sentence.
- **GPU without setup.** Whisper `large-v3-turbo` runs through whisper.cpp on
  Vulkan, which works with the graphics driver you already have (NVIDIA, AMD,
  Intel) and falls back to the CPU when there is no GPU.

## Install

Download `ai-voice-dictation_<version>_amd64.deb` from
[Releases](https://github.com/zhangxingeng/ai_voice_dictation/releases) and open it, or:

```sh
sudo apt install ./ai-voice-dictation_*_amd64.deb
```

Launch **AI Voice Dictation** from the app grid. The first launch downloads the speech
model (574 MB) and registers the shortcut with GNOME. To change the key, edit
it in *Settings → Keyboard → Custom Shortcuts*; the app will not change it back.

**Updates:** when a newer release exists, an **Update** button appears in the
window. It downloads the new `.deb` and installs it after a password prompt;
restart the app to use it.

To remove the shortcut: `ai_voice_dictation --unbind`. To remove the app:
`sudo apt remove ai-voice-dictation`. The model lives in `~/.local/share/ai_voice_dictation/`.

The global shortcut needs GNOME. Elsewhere the app works, but you trigger it by
binding `ai_voice_dictation --toggle` to a key yourself.

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
DICTATION_WAV=clip.wav [DICTATION_LANG=zh] cargo test --release -- --ignored   # real model, 16 kHz mono WAV
```

Release: push a `v*` tag. CI builds the `.deb` and attaches it to a release.

See [`STATE.md`](STATE.md) for the design and the measurements behind it.
