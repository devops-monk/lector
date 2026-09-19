# lector

Local-only text-to-speech for macOS. Select text anywhere, hear it read aloud.
Nothing leaves your machine: inference runs in-process via sherpa-onnx, with no
Python, no sidecar, and no network at runtime.

## Status

Phase 0 (engine spike) complete. Measured on an M1 Pro with
`vits-piper-en_US-amy-medium-int8`:

| | |
|---|---|
| RTF | 0.15 (verba's published figure: 0.14) |
| Model load | ~820 ms |
| Time to first audio | ~470 ms |

Streaming synthesis and mid-utterance cancellation both verified:

```
cargo run -p lector-engine --bin spike -- "text to speak"
cargo run -p lector-engine --bin spike -- --cancel 1 "aborts after one sentence"
```
