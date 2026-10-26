# ADR-177: spectrum, a consumer of the PlutoSDR stream

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `spectrum` 0.1.0

## Context

`sdr-pluto-rx` (ADR-176) owns the radio and streams `iq.cs16.v1` on the node's cogbus with the samples it lost counted. The Ventuno cog plan (P7, phase R3) wants a 2.4 GHz occupancy reading per minute with the loss reported, and a spectrum for the cogs that display or decode: `spectrum.psd.v1` and a band occupancy reading. The radio is rated 325 to 3800 MHz and run as 70 to 6000 MHz, and the owner takes turns between bands, so a consumer sees a stream whose band changes.

## Decision

1. **A pure consumer.** The cog never opens the radio. It subscribes to `iq.cs16/<source>` with the `drop-oldest` policy (the owner's writer never waits for it), takes the new ring when the broker announces one after a restart, and reconnects with backoff. It speaks the cogbus protocol directly (`src/bus.rs`, `src/ring.rs`) so it stays a standalone package.
2. **Welch spectrum.** 1024-point FFT (Hann, 50 percent overlap) on f32, written in the cog (`src/fft.rs`, checked against a direct DFT), averaged per window (2 s) and per band segment: a retune ends the window. The published spectrum is `spectrum.psd.v1` (a registered reading): `center_hz`, `span_hz` (the sample rate), 256 bins of dBFS, lowest frequency first, rounded to 0.1 dB so the reading stays near 3 KB. Levels are dBFS with a full-scale complex tone in one bin at 0 dB; they are not dBm.
3. **Occupancy.** Every fourth frame the cog takes the median bin power of the usable band (the analog bandwidth over the sample rate) as the noise floor and counts a bin busy when it is more than 10 dB above it. Cells are counted in eight equal slices of the usable band. Per band (centre and rate) the counts add up over the interval (60 s) and are published as one `telemetry.v1` reading per band: busy share overall and per slice, mean floor, peak, the share of the interval the radio spent on the band (`coverage`), and the loss counts. Occupancy is relative to each frame's own floor, so a band that is busy everywhere reads low; the guide says so.
4. **Loss is counted, never hidden.** Three kinds are kept apart and kept out of the spectrum: samples the owner declared lost (a gap record, or `lost` on a chunk, counted once when both say the same), samples this reader missed (a jump of the sample index beyond what the owner declared, which is what a ring overwritten under a slow reader looks like) and samples from a dead all-zero receive path. Each gap or jump also breaks the overlap between FFT frames, so no frame straddles missing samples. Every reading carries `samples_used`, `samples_lost`, `samples_skipped`, `samples_dead` and `loss_fraction`.
5. **Uncalibrated outside the rated range.** Bands outside 325 to 3800 MHz carry `uncalibrated` (a field of the spectrum, an item of the occupancy).
6. **Asking for bands.** `--ask-plan` sends the owner a `plan` call (id `spectrum`) with the bands given, so two consumers can share the tuner by band plan; the owner refuses what it cannot run and the cog logs the reason and goes on with what it is given.
7. **Contract.** stdout JSON per interval, an 8-float store vector (id 30: centre GHz, rate MS/s, occupancy, floor, peak, loss fraction, windows published, uncalibrated), `/status /raw /guide /healthz` on `127.0.0.1:8064` (`/raw` is the newest spectrum reading), `--once --simulate` (a synthetic band marked `SYNTHETIC`) and `--once --replay FILE` (a capture made with the owner's `--record`). The windows and intervals of an offline run follow the stream's own time, so a run is deterministic.
8. **Schemas.** It uses only registered schema ids: `iq.cs16.v1` in, `spectrum.psd.v1` and `telemetry.v1` out. The call to the owner uses `sdr.v1`, which WeftOS has to register (ADR-176).

## Consequences

- At 5 MS/s the cog uses about half a core on the Ventuno Q (measured over the overnight soak, size-optimized build). Most of that is reading the stream, because each 8192-sample record is a 44 KB base64 JSON message; the first build used a whole core and missed 1.6 percent of the stream until the base64 decode got a lookup table. A reader that cannot keep up loses records rather than slowing the owner, and says so in `samples_skipped`.
- A band visited rarely, because the owner is busy with other plans, gets a short `coverage` and few frames; the reading says how few.
- `rf-census` and other decoders can take `band.occupancy` and `spectrum.psd` without touching the stream.

## Verification

Unit tests cover the FFT against a direct DFT and tones in their bins, the Welch levels (a full-scale tone reads 0 dBFS, a half-scale tone 6 dB down, negative offsets below the centre), frames continuing across pushes and restarting after a break, noise not reading busy and a strong tone landing in the right slice, decimation, the collector (windows by time and at retunes, the readings against the registered schema rules, per-band occupancy with coverage, a declared gap counted once, a jump the owner did not declare counted as this reader's miss, a dead path kept out, a quiet stream flushing on a tick), the capture reader, and the whole pipeline on the synthetic band. `tests/cli.rs` runs the binary: `--help`, `--once --simulate`, replay of recorded fixtures (2.4 GHz, 1090 MHz and FM captured from the Pluto), and the bus loop against a stand-in broker with a ring passed as a descriptor.
