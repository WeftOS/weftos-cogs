# ADR-184: tts, speech on the audio.pcm stream

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `tts` 0.1.0

## Context

The voice loop ends in sound. `audio-out` (ADR-178) already plays `audio.pcm/<source>`; something has to write that stream. The board has Kokoro and Piper (sherpa-onnx) on the CPU, measured in the bench; Phonon (`ptts`) is a candidate for later.

## Decision

1. **Text in, stream out.** The cog subscribes to `transcript/<source>` (default `reply0`) and writes `audio.pcm/<name>` (default `tts0`), 640-byte records of 16 kHz mono s16le, the same stream format as the microphone.
2. **A worker process** (`worker/tts_worker.py`, carried in the binary, run with `$TTS_PYTHON`) loads Piper (default) or Kokoro (`--engine`), speaks a warm-up sentence, then synthesizes on request, writing mono float32 samples to a file and answering with their rate and count.
3. **Resampling in the cog** (`src/resample.rs`): windowed-sinc (Hann, 16 taps each side, cutoff lowered going down) from the engine's rate (24 000 or 22 050 Hz) to 16 000, then clipped to s16le, padded with silence to whole chunks. Tests hold pitch and level of a 1 kHz tone and show a 10 kHz tone removed rather than folded.
4. **Real-time writes.** One chunk every 20 ms with a drift-free pacer, so `audio-out` receives speech at the rate it plays it and `drop-oldest` never skips. Nothing is written when idle.
5. **Queue.** Text that arrives while speaking waits; more than four waiting drops the oldest and counts it. Text is made speakable (control characters to spaces, whitespace collapsed, 600 characters).
6. **Fail honestly.** A worker that is not ready in two minutes is `no_source` with the reason and is retried every 5 s. A failed synthesis is counted and kept as `last_error`.
7. **`--simulate`** speaks an 80 ms tone per word; `--replay FILE` speaks JSON-lines text.

## Consequences

- Measured on the board at three threads: Piper makes speech in 0.11 of its length, Kokoro in 1.09, so Piper is the default and Kokoro is slower than real time unless given more threads. There is no streaming synthesis; speech starts when the whole sentence is made.
- Speech that has begun cannot be interrupted.
- One voice engine runs at a time; the worker holds the model (a few hundred MB).
- `interfaces = ["none"]`: the speaker belongs to `audio-out`.

## Verification

Unit tests cover resampling (lengths, pitch and level at both rates, aliasing, clipping), the worker protocol against a stand-in process, option bounds, speakable text, rendering to whole chunks, synthesis failures, the report and vector. `tests/cli.rs` runs the binary with `--simulate`, a missing worker and a windowed run. The on-board run (text in, speech out, scored by round-trip recognition) is in the card.
