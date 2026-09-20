# Lector

**Local-only text to speech for the desktop.** Select text anywhere, press a key,
hear it read aloud. Nothing you select ever leaves your machine.

Lector is the mirror image of [Vox](https://github.com/devops-monk/vox): Vox turns
speech into text with a global hotkey; Lector turns text into speech with one.
Same shape, pointed the other way.

---

## Why it is unusual

Most local TTS apps ship a Python sidecar, or call a cloud API, or both. Lector
does neither:

- **No Python.** Inference is [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
  linked statically into the binary. The phonemizer (espeak-ng) is inside that
  static library, so there is not even a subprocess.
- **No network at runtime.** Models are downloaded once, verified, and used
  offline forever after. Turn off Wi-Fi and it still works.
- **No sidecar and no IPC for audio.** The model and the audio device live on the
  same thread, so synthesized samples go straight into the ring buffer the sound
  card is draining. Audio never crosses a process or IPC boundary.

## Features

### Speaking

- **Global hotkey** — `Option+Shift+Space` speaks the current selection in any
  app. Press again to stop.
- **macOS Services menu** — right-click selected text → *Speak with Lector*. No
  Accessibility permission and no clipboard round-trip, because macOS hands the
  text over directly.
- **Stops mid-word.** Cancellation is checked inside the inference loop, so stop
  means *now*, not at the end of the sentence.
- **Interrupts cleanly.** Pressing the hotkey while speaking cancels the old
  utterance and starts the new one; measured at ~55 ms.
- **Adjustable speed** — 0.8x to 2x, applied at synthesis time so pitch is
  unaffected.

### Reading well

Text is not fed to the model raw. It is turned into something worth listening to:

- Fenced code becomes the spoken phrase *"Code block."*, announced once per run
  rather than once per fence.
- Markdown tables become *"Table omitted."* — reading one aloud cell by cell is
  unbearable.
- File paths shorten to their basename and URLs to their hostname, so a voice
  does not spend twenty seconds spelling out `/Users/.../src/components/`.
- Headings and list items get a full stop, so the engine gives them falling
  intonation instead of running them together.
- Emphasis markers, link URLs and emoji are stripped; inline code keeps its
  content.
- Which of these apply is per-context: aggressive for narrating an agent, gentle
  for a document, off entirely for a script someone wrote to be spoken.

### Voices

- **Piper** (21 MB) — fast and small, ready seconds after first launch.
- **Kokoro** (349 MB) — noticeably better, five English speakers
  (`af_heart`, `af_bella`, `am_michael`, `bf_emma`, `bm_george`).
- Downloaded from the menu bar with progress, verified by SHA-256, and unpacked
  atomically. A model that fails its checksum installs **nothing** — no
  directory, no partial file.
- British voices automatically get the British pronunciation table; the rest get
  the American one.

### Menu bar

No window at all. The menu tells you what it cannot otherwise show:

- one item that changes verb between *Speak Selection* and *Stop Speaking*
- a line that says Accessibility permission is missing, and opens the prompt
- a line that says no voice is installed
- voice and speed pickers, with the current choice checked

## Performance

Measured on an Apple M1 Pro. RTF is real-time factor: 0.15 means a second of
speech takes 0.15 seconds to generate.

| | Piper (int8) | Kokoro (fp32) |
|---|---|---|
| Real-time factor | **0.15** | **0.27** |
| Model load | ~820 ms | ~860 ms |
| Time to first audio | **~470 ms** | ~800 ms |
| Download | 21 MB | 349 MB |

Both beat real time comfortably. Piper is the default because first-audio latency
is what a hotkey feels like.

> On Apple silicon, Kokoro's **int8 build is slower than fp32** (0.96 vs 0.41 in
> published figures) — ARM pays a dequantisation tax the smaller weights never
> earn back. Piper is the opposite. Re-measure before "optimising" either.

## Install

Download from [Releases](https://github.com/devops-monk/lector/releases):

| Platform | File |
|---|---|
| macOS (Apple silicon) | `Lector-*-macos-arm64.dmg` |
| macOS (Intel) | `Lector-*-macos-x64.dmg` |
| Windows | `Lector-*-windows-x64.exe` or `.msi` |
| Linux | `Lector-*-linux-x64.AppImage` or `.deb` |

Builds are **unsigned**. macOS will refuse to open it the first time: right-click
the app and choose *Open*, or run
`xattr -dr com.apple.quarantine /Applications/Lector.app`. Windows SmartScreen
warns for the same reason.

### Platform support, honestly

The engine is portable and CI builds and tests all four targets. But Lector has
only been *used* on macOS (Apple silicon). The Services menu is macOS-only, and
the Windows and Linux builds have not been run by a human. Treat them as
untested.

## Build from source

Needs Rust and, on Linux, some system libraries.

```sh
git clone https://github.com/devops-monk/lector && cd lector
cargo run -p lector
```

<details>
<summary>Linux system dependencies</summary>

```sh
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  patchelf libxdo-dev libasound2-dev build-essential file
```

`libxdo` is for synthesizing the copy keystroke; `libasound2` is for audio.
</details>

The first build downloads a prebuilt sherpa-onnx static library (~25 MB, added to
the binary). No cmake, no C++ compile, ~30 s cold.

On first launch there are no voices; use the menu bar to download one. The hotkey
needs Accessibility permission — launched from a terminal that already has it,
the binary inherits it; a bundled app asks for its own.

### Exercising the pieces

```sh
cargo test --workspace                      # 32 tests
cargo test --workspace -- --ignored         # + real model downloads

cargo run -p lector-text --example demo     # see how text is chunked
cargo run -p lector-engine --bin demo       # latency, interrupt, stop
cargo run -p lector-engine --bin spike      # RTF and streaming, one model

LECTOR_MODEL=models/kokoro-multi-lang-v1_0 LECTOR_SID=3 \
  cargo run -p lector-engine --bin spike    # ...or another

python3 scripts/make-icons.py               # regenerate icons
./scripts/verify-catalog.sh                 # re-check model checksums
```

## Architecture

```
crates/lector-text     markdown -> speakable chunks.  Pure, 29 tests.
crates/lector-engine   catalog, installer, synthesis actor, audio output.
src-tauri              tray, hotkey, selection capture, Services menu.
```

**One thread owns the model and the audio device.** They have to share one anyway
(`cpal::Stream` is `!Send` on macOS), and pairing them makes the hot path
lock-free: the model's per-sentence callback resamples straight into the ring the
device is draining, so sentence one is audible while sentence two is still being
generated.

**Cancellation is a generation counter, not a flag.** A bool cannot express
"cancel what is in flight but not what I just enqueued" — which is exactly the
race an interrupting hotkey press creates. The counter is read once per sentence
inside the inference loop.

**Playback is native, not in a webview.** Two of the four planned surfaces have no
window, so there would be no `<audio>` element to use. It also avoids the wall
every webview-based TTS app hits: autoplay policy, throttled background timers,
and audio marshalled over IPC.

**Chunking balances two opposing pressures.** A sentence synthesized alone gets no
prosodic context, so a bare "Okay." lands as a clipped bark — hence a 100-char
floor. But the first chunk sets the latency you feel, so it is exempt and goes out
as one sentence however short.

**Audio seams are handled deliberately.** Each chunk is trimmed of the model's
baked-in silence, faded 10 ms at the cut, DC-corrected, and followed by a 150 ms
gap of our choosing. Per-chunk peak normalisation is deliberately *absent*: it
makes loudness pump between sentences by boosting quiet ones to match loud ones.

## Roadmap

Lector is built in phases. The first two are done.

- [x] **Engine** — streaming synthesis, immediate cancellation, native playback
- [x] **Hotkey** — tray, global hotkey, Services menu, selection capture
- [x] **Voices** — catalog, verified downloads, Kokoro, voice and speed pickers
- [ ] **Narrate an agent** — speak Claude Code's output by tailing its transcripts
- [ ] **Long-form listening** — import EPUB and Markdown, queue, resume, bookmarks
- [ ] **Studio** — script editor, per-paragraph voices, export to WAV

Deliberately out of scope: voice cloning (needs a model class that would break
the no-Python rule), speech-to-text (that is Vox), and any cloud fallback tier
(it would contradict the entire premise).

## Releasing

```sh
./scripts/release.sh 0.2.0        # bumps versions together, tags
git push origin main && git push origin v0.2.0
```

The pipeline refuses to build a tag whose version disagrees with `Cargo.toml`,
builds all four platforms, smoke-tests each bundle, and opens a **draft** release
for review.

## Credits

Built on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) by the k2-fsa
project, with [Piper](https://github.com/rhasspy/piper) and
[Kokoro](https://huggingface.co/hexgrad/Kokoro-82M) voices.

The design owes a lot to reading [verba](https://github.com/nuvocode/verba)
(in-process sherpa-onnx with no sidecar, and the model installer),
[companion-tts](https://github.com/kleenpulse/companion-tts) (the prefetch queue
and its invariants), [readest](https://github.com/readest/readest) (trim the
engine's silence and insert your own gap), and
[vocal](https://github.com/skartik-sk/vocal) (the Services menu as a zero-window
entry point).

## License

MIT
