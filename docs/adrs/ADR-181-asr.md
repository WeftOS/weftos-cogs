# ADR-181: asr, speech recognition on the node

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `asr` 0.1.0

## Context

The hear-and-answer loop (Ventuno cog plan 5.5) starts with text. The brick spike measured Parakeet TDT 0.6B int8 on the board's CPU at 8.3 times real time with word-for-word transcripts, and showed that the NPU keyword model alone fires on ordinary speech. The NPU is shared and scheduled by `npu-infer`; recognition on the CPU takes no NPU time. `audio-in` (ADR-177) provides the stream.

## Decision

1. **Input.** The cog reads the stream `audio.pcm/<source>` (default `mic0`) as an ordinary reader (`drop-oldest`); it owns no device, so many listeners may read the same microphone.
2. **First stage: energy.** `src/segment.rs` cuts the 20 ms chunks into utterances. The room is learned for the first second (the mean level is the floor); afterwards the floor falls at once to a quieter chunk and rises 0.05 dB per chunk while nobody is speaking. Speech is `floor + 10 dB`, never below -45 dBFS. An utterance carries 240 ms before its first loud chunk, ends after 600 ms of quiet, is dropped if under 300 ms of speech, and is cut at 15 s. All of it is settable.
3. **Second stage: the transcript.** Each utterance goes to a Parakeet worker process (sherpa-onnx, int8, `src/worker.rs`): the cog writes the utterance to one of four reused files and sends the path as a JSON line; the worker answers the text. The worker is a Python script the binary carries (`worker/asr_worker.py`), run with `$ASR_PYTHON` against the model directory. Empty text (a cough, a door) is dropped and counted as `empty`.
4. **Output.** `transcript.v1` on `transcript/<name>` (default `asr0`): the text, `lang` en, the utterance's start and end on the stream's clock, `confidence` 1.0 (Parakeet reports none; 1.0 means not measured, and the guide says so).
5. **Fail honestly.** A worker that does not become ready in two minutes is `no_source` with the reason. A recognition error is counted, kept as `last_error` and the worker keeps serving.
6. **`--simulate`** runs two tone bursts through the segmenter and a recognizer that reports the length; `--replay FILE` runs a WAV or raw file through the real recognizer, unpaced.

## Consequences

- A model worker in Python is a dependency on the node (`sherpa_onnx`, `numpy`, the model files), the same kind as `npu-infer`'s GenieX. The cog's `interfaces` is `["none"]`: it opens no device.
- Recognition is serial. An utterance in progress delays the next; the stream's ring (about 130 s) absorbs it.
- The first second after start hears nothing.
- English only (the v2 model).

## Verification

Unit tests cover the segmenter (burst with preroll and hangover, a click, a short pause, the maximum, a steady noise raising the floor, flush), the worker protocol against a stand-in process, the engine (two bursts, empty text, worker errors), the transcript body, the report and vector. `tests/cli.rs` runs the binary with `--simulate`, a missing worker and a replayed file through a stand-in worker. The on-board run (the Mac speaker's phrase through `audio-in` and this cog to a transcript) is in the card.
