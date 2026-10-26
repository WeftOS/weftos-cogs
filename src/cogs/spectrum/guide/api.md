# API

> Topics, the HTTP surface and the store vector.

## Input

`iq.cs16/<source>` (`iq.cs16.v1`, stream), read with the `drop-oldest` policy: if the cog falls behind, records are overwritten and the missing samples are counted as `samples_skipped`.

## Output

`spectrum.psd/<source>` (`spectrum.psd.v1`, reading) every window; `band.occupancy/<source>` (`telemetry.v1`, reading) every interval, one per band.

## HTTP (loopback)

`/status`, `/raw` (the newest spectrum reading), `/guide`, `/healthz`.

## Store vector (id 30)

`[centre GHz, rate MS/s, occupancy overall, floor dBFS, peak dBFS, loss fraction, windows published, uncalibrated]`.
