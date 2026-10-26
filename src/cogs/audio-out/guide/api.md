# API reference

> The stream, the report, the store vector, the export endpoints and the settings.

## Stream read: `audio.pcm/<source>`

Schema `pcm.s16le.v1`, class `stream`. Each record is 640 bytes: 320 mono samples of signed 16-bit little-endian PCM at 16 kHz (20 ms). A record of any other length is skipped. `seq` counts from 1 and a gap is a drop.

## Reading `telemetry/<name>`

`telemetry.v1`: `chunks.played`, `chunks.dropped`, `write.errors`, `latency` (ms), `level.rms` (dBFS), `stream.seq`, `bus.connected`.

## stdout report (one per interval)

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `sink`, `stream` | string | `alsa:<device>` or `null`; `audio.pcm/<source>` |
| `chunks_played`, `chunks_per_s` | numbers | 50 per second when a writer sends real time |
| `dropped_chunks`, `write_errors` | numbers | |
| `latency_ms` | number/null | mean time from the writer's write to this cog's hand-off to ALSA |
| `rms_dbfs`, `peak_dbfs` | number/null | level of what was played |
| `idle` | bool | healthy but nothing arrived in the window |
| `bus` | object | `connected`, `stream_open`, `last_seq`, `error` |
| `error` | string | only with `no_source` |

## Store vector (id 30)

Eight floats; a source that is absent is **-1**.

| Slot | Name | Unit |
|---|---|---|
| 0 | `rms_dbfs` | dBFS |
| 1 | `peak_dbfs` | dBFS |
| 2 | `chunks_per_s` | per second |
| 3 | `dropped` | chunks |
| 4 | `write_errors` | count |
| 5 | `bus_connected` | 0 or 1 |
| 6 | `latency_ms` | ms, -1 when nothing was played |
| 7 | `active` | 1 if anything was played |

## Export (`http://127.0.0.1:8064`, continuous mode)

`/status` latest report, `/raw` the latest report's raw view, `/history` the last 60 reports, `/guide` this guide, `/healthz`.

## Settings

`--interval`, `--window`, `--source`, `--name`, `--card-name`, `--device`, `--api-bind`, `--replay`, `--simulate`.
