# ADR-178: audio-out, the node's speaker owner

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `audio-out` 0.1.0

## Context

Speech synthesis, alerts and a future assistant all want to speak, and an ALSA playback device is not shared without a mixer. The Ventuno cog plan (5.2) makes one cog the owner of playback; WeftOS ADR-122 section 4b is the ownership rule and the `audio.pcm` stream (`pcm.s16le.v1`, 20 ms chunks) is the way audio reaches it.

## Decision

1. **One owner.** `[[devices]] path = "/dev/snd/pcmC*D*p"`. Capture and playback nodes do not overlap, so `audio-in` and `audio-out` coexist; two playback owners do not.
2. **Input.** The cog subscribes to `audio.pcm/<source>` (default `tts0`, `--source`), declared as `[[inputs]]` with `drop-oldest`. The default is not the microphone's source, so the speaker never plays the microphone back by default. One writer per stream; mixing two writers is out of scope.
3. **Playback by `aplay`.** The cog starts `aplay -D plughw:<c>,<d> -t raw -f S16_LE -r 16000 -c 1 -` and writes each 640-byte record to its stdin. The pipe blocks while ALSA's buffer is full, which keeps playback at real time. A record of another length is skipped.
4. **Device choice** is the same as `audio-in`: from `WEFTOS_DEVICES`, by `--card-name`, or `--device` by hand.
5. **Drops are counted.** A gap in the stream's `seq` is added to `dropped_chunks`; the cog plays on from the oldest record still safe to read. Latency is the time from the writer's wall clock stamp in the record to the hand-off to ALSA.
6. **Fail honestly.** No handed playback node, no `aplay`, or no broker is `no_source` with the reason. A running cog with no writer yet is `ok` with `idle: true`.
7. **`--simulate`** plays a tone into a null sink and opens no device; `--replay FILE` plays a WAV or raw file through the real sink. Reports are marked.

## Consequences

- There is no jitter buffer beyond ALSA's; a writer that sends in bursts will stutter. A buffer is a later addition if a TTS cog needs it.
- The ring code is shared (as a copy) with `audio-in` and `camera`; a fix in one is a fix in all four cogs that carry `src/bus.rs`.
- `interfaces = ["audio-out"]`.

## Verification

Unit tests cover playback-node parsing, device choice, level, WAV unwrapping, the null sink, options and the report (drops, latency, idle, no_source). `tests/cli.rs` runs the binary with `--simulate`, `--replay` and no device. The ring and broker-protocol tests are those of `audio-in` (same `src/bus.rs`). The on-board run (a stream written by a test writer and played, scored by ear against the known text) is in the card.
