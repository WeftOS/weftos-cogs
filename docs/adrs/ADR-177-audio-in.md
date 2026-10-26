# ADR-177: audio-in, the node's microphone owner

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `audio-in` 0.1.0

## Context

The Ventuno cog plan (5.2) gives every device one owning cog. Speech recognition, wake-word, level and recording cogs all want the microphone, and an ALSA capture device can be opened by one reader at a time through `hw:` and shared only through a sound server the board does not run. WeftOS ADR-122 supplies the carrier (a shared-memory stream, `audio.pcm/<source>`, schema `pcm.s16le.v1`) and section 4b supplies the ownership rule (`[[devices]]` in `cog.toml`, one owner per node, `WEFTOS_DEVICES` handed to the owner).

## Decision

1. **One owner.** The cog declares `[[devices]] path = "/dev/snd/pcmC*D*c"` (read-only). The node refuses a second cog that claims a capture node or names one in its arguments. On a shared-uid node this is advisory, and the guide says so.
2. **Only what was handed.** The cog opens the capture node the host put in `WEFTOS_DEVICES`, picked by card name (`--card-name`, a substring of the name in `/proc/asound/cards`) when several were handed, else the lowest card. `--device` names an ALSA device by hand for a bench run and is the one way around the handed list.
3. **Capture by `arecord`.** The cog runs `arecord -D plughw:<c>,<d> -t raw -f S16_LE -r 16000 -c 1` and reads its stdout in 640-byte chunks (20 ms). ALSA's `plughw` layer converts the card's native format, so the cog needs no resampler. The child dies with the cog.
4. **The stream.** The cog opens `audio.pcm/<source>` (default `mic0`) as the stream writer through the broker: `stream_open` with `max_record` 640, the ring received over `SCM_RIGHTS` and mapped, records written with the capture clock as `t_ns`. The writer does not wait for `block` readers (it does not map their cursors), so a slow `block` reader loses records like a `drop-oldest` one. The cog speaks the broker protocol itself (`src/bus.rs`), like `npu-infer`, to stay a standalone package; the ring code is a port of the broker's, with a round-trip test against a broker stand-in.
5. **Fail honestly.** No handed capture node, no matching card, no `arecord` or a capture that ends is `no_source` with the reason, an all-`-1` vector, and nothing posted to the store. A silent card reads -120 dBFS, which is reported as such, not as a quiet room.
6. **Reporting.** Per interval: level (mean-square RMS over the window, peak, clipped samples), chunks per second (50 when healthy), short reads, bus state, and an 8-float vector (id 29). A `telemetry/<source>` reading carries the same counters.
7. **`--simulate`** publishes a tone with noise paced to real time (a drift-free pacer); `--replay FILE` plays a WAV or raw file through the same path. Both are marked in `provenance`.

## Consequences

- Latency is `arecord`'s buffer plus a pipe; it is not tuned. A wake-word cog that needs less should ask for a smaller `--buffer-time`, which is not exposed yet.
- The stream is 16 kHz mono only. A stereo or higher-rate consumer needs a second source or a later schema.
- Another process that opens the card directly (not through the ledger) can make `arecord` fail with "device busy"; the report then says `the capture ended`.
- `interfaces = ["audio-in"]`; it opens no other device.

## Verification

Unit tests cover device selection from the handed list and card names, chunk level in dBFS, WAV unwrapping, option bounds, the report and vector, telemetry names, and the stream: ring write and read in order, a reader a ring behind jumping forward, and a broker stand-in that passes the ring by descriptor and a pushed call. `tests/cli.rs` runs the binary with `--simulate`, `--replay` and no device. WeftOS `tests_device_owner.rs` covers the ownership rules. The on-board run (stream to a reader, the Mac speaker as the source, the second cog refused) is in the card.
