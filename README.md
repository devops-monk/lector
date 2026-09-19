# lector

Local-only text-to-speech for macOS. Select text anywhere, hear it read aloud.
Nothing leaves your machine: inference runs in-process via sherpa-onnx, with no
Python, no sidecar, and no network at runtime.

## Status

Phase 1: the hotkey surface works. Tray-only, no window.

Select text in any app and press **Option+Shift+Space** to hear it; press again
to stop. Right-click → Services → *Speak with Lector* does the same thing
through a second entry point (once run as a bundled .app).

Measured on an M1 Pro with `vits-piper-en_US-amy-medium-int8`:

| | |
|---|---|
| RTF | 0.15 (verba's published figure: 0.14) |
| Engine ready at launch | ~1.0 s (audio device + model warm) |
| First audible sample | ~470 ms |
| Interrupt accepted | ~55 ms |

### Running it

The model is not vendored yet -- phase 2 adds the catalog and downloader. For now:

```sh
mkdir -p models && cd models
curl -L -O https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-amy-medium-int8.tar.bz2
shasum -a 256 vits-piper-en_US-amy-medium-int8.tar.bz2
# expect bd23c0aa629eb3719448582f45ede49e8fa6a679061fed5eab16a6a6fd8e7e82
tar xjf vits-piper-en_US-amy-medium-int8.tar.bz2 && cd ..

cargo run -p lector
```

The hotkey needs Accessibility permission to read the selection. Launched from a
terminal that already has it, the binary inherits it; a bundled .app asks for its
own.

### Exercising the pieces directly

```sh
cargo test -p lector-text                          # 29 tests, pure functions
cargo run -p lector-text --example demo            # see how text is chunked
cargo run -p lector-engine --bin demo              # latency, interrupt, stop
cargo run -p lector-engine --bin spike -- --cancel 1
```

## Architecture

Inference is sherpa-onnx, linked statically -- no Python, no sidecar, no
phonemizer subprocess. Playback is cpal on the same thread as the model, so audio
never crosses an IPC boundary; two of the four planned surfaces have no window to
play it in.

- `crates/lector-text` -- markdown to speakable chunks. Pure, tested.
- `crates/lector-engine` -- the synthesis actor, audio output, resampling.
- `src-tauri` -- tray, global hotkey, selection capture, Services menu.
