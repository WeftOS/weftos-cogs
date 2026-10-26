# API reference

> The stream, the report, the store vector, the export endpoints and the settings.

## Stream `audio.pcm/<source>`

Schema `pcm.s16le.v1`, class `stream`. Each record is 640 bytes: 320 mono samples of signed 16-bit little-endian PCM at 16 kHz (20 ms). `t_ns` is the capture clock (Unix nanoseconds when the chunk was read); `seq` counts from 1 and a gap is a drop.

## Reading `telemetry/<source>`

`telemetry.v1`: `chunks.total`, `chunks.short`, `level.rms` (dBFS), `level.peak`, `level.clipped`, `stream.seq`, `bus.connected`.

## stdout report (one per interval)

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `source` | string | `alsa:<device>`, `simulated`, `replay`, `none` |
| `rate_hz`, `channels`, `chunk_ms`, `chunk_bytes` | numbers | 16000, 1, 20, 640 |
| `chunks`, `chunks_per_s` | numbers | chunks in the window and their rate (50 when healthy) |
| `rms_dbfs`, `peak_dbfs` | number/null | window level |
| `clipped_samples`, `short_reads` | numbers | |
| `devices` | array | what the host handed |
| `bus` | object | `connected`, `topic`, `seq`, `write_errors`, `error` |
| `error` | string | only with `no_source` |

## Store vector (id 29)

Eight floats; a source that is absent is **-1**.

| Slot | Name | Unit |
|---|---|---|
| 0 | `rms_dbfs` | dBFS |
| 1 | `peak_dbfs` | dBFS |
| 2 | `clipped` | samples |
| 3 | `chunks_per_s` | per second |
| 4 | `short_reads` | count |
| 5 | `bus_connected` | 0 or 1 |
| 6 | `stream_seq` | last record seq |
| 7 | `rate_ratio` | chunks_per_s / 50, capped at 2 |

## Export (`http://127.0.0.1:8063`, continuous mode)

`/status` latest report, `/raw` the latest chunk (`pcm_b64`), `/history` the last 60 reports, `/guide` this guide, `/healthz`.

## Settings

`--interval`, `--window`, `--source`, `--card-name`, `--device`, `--api-bind`, `--replay`, `--simulate`.
