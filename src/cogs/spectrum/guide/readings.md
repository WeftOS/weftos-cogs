# Readings

> What the two readings contain and how to read them.

## `spectrum.psd/<source>` (`spectrum.psd.v1`)

The registered reading: `center_hz`, `span_hz` (the sample rate, the whole width the bins cover), and `bins`, 256 values in dBFS from the lowest frequency to the highest, rounded to 0.1 dB. The analog filter rolls off at the edges, so the usable part is the middle `bandwidth_hz / sample_rate_hz` of the bins. A full-scale complex tone in one FFT bin reads 0 dBFS.

Added fields: `bandwidth_hz`, `fft_size` (1024), `frames` averaged, `integration_ms`, `floor_dbfs` (median level), `peak_dbfs`, `uncalibrated`, and the loss counts: `samples_used`, `samples_lost`, `samples_skipped`, `samples_dead`, `loss_fraction`.

## `band.occupancy/<source>` (`telemetry.v1`)

One reading per band per minute, as named items. `band.centre`, `band.rate`, `band.bandwidth` say which band. `occ.overall` and `occ.slice0` to `occ.slice7` are the busy share overall and in eight equal slices of the usable band (0 to 1). `floor` and `peak` are the mean median level and the highest bin in dBFS. `occ.threshold` is the busy threshold above the floor (10 dB).

`coverage` is the share of the minute the radio spent on this band. When the owner takes turns between bands, each band's reading covers only its own dwells, and `coverage` shows how much. `samples.*` and `loss.fraction` are the loss accounting for the minute.
