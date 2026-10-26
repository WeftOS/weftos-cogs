# API

> What the cog publishes and what it answers.

## Stream: `iq.cs16/<source>` (`iq.cs16.v1`)

One ring record is a 40-byte header and then complex samples (little-endian i16 I, Q; 12 significant bits). The header carries the segment (one per dwell), the centre, rate and bandwidth, the sample index on the segment's clock, the samples lost just before it, and flags (discontinuity, band changed, all zero). A gap is also written as its own record with no samples. IQ stays on the node; it is not exported by default.

## Reading: `sdr.status/<source>` (`sdr.status.v1`)

`telemetry.v1`-style items: LO, rate, bandwidth, gain, RSSI, temperatures, delivered, expected, lost, pending, gap events, overflows, zero blocks, retunes.

## Service: `svc.sdr/<source>`

`tune`, `set_rate`, `set_bw`, `set_gain`, `plan`, `plan_remove`, `lease`, `release`, `status`, and `tx_config`, which is always refused. Tuning refers to the caller's own plan; several callers' plans take turns. A lease is exclusive for up to an hour.

## HTTP (loopback)

`/status`, `/raw?samples=N`, `/guide`, `/healthz`.

## Store vector (id 29)

`[centre GHz, rate MS/s, power dBFS, loss fraction, PHY temp C, Zynq temp C, zero-block fraction, gap events]`.
