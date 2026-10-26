# API reference

> The stream, the report, the store vector, the export endpoints and the settings.

## Stream `audio.pcm/<name>`

`pcm.s16le.v1`, class `stream`: 640-byte records (20 ms of 16 kHz mono s16le) while speaking, none when idle. `t_ns` is the wall clock when the chunk was written.

## Reading `telemetry/<name>`

`telemetry.v1`: `spoken`, `dropped`, `failures`, `audio` (s), `synth.time` (s), `stream.seq`, `bus.connected`.

## stdout

`{"speaking": "...", "audio_ms": N}` per text, then one report per interval:

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `voice` | string | the engine, `synthetic`, `none` |
| `spoken`, `dropped`, `failures` | numbers | since start |
| `audio_s`, `real_time_factor` | numbers | speech produced; synthesis time over speech time |
| `queued`, `speaking` | number, bool | |
| `last_text`, `last_error` | string/null | |
| `bus` | object | `connected`, `stream_open`, `seq`, `error` |

## Store vector (id 36)

Eight floats; a source that is absent is **-1**.

| Slot | Name |
|---|---|
| 0 | `spoken` |
| 1 | `rtf` |
| 2 | `audio_s` |
| 3 | `dropped` |
| 4 | `failures` |
| 5 | `queued` |
| 6 | `bus_connected` |
| 7 | `stream_seq` |

## Export (`http://127.0.0.1:8070`, continuous mode)

`/status`, `/raw`, `/history`, `/guide`, `/healthz`.

## Settings

`--interval`, `--window`, `--source`, `--name`, `--engine`, `--speaker`, `--speed`, `--threads`, `--python`, `--models-root`, `--api-bind`, `--replay`, `--simulate`.
