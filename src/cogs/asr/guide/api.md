# API reference

> The readings, the report, the store vector, the export endpoints and the settings.

## Reading `transcript/<name>`

`transcript.v1`, class `reading`: `source_id`, `node_id`, `t_ns` (utterance start), `seq`, `text` (at most 4096 chars), `lang`, `start_ns`, `end_ns` (capture clock of the stream), `confidence` (always 1.0: not measured).

## Reading `telemetry/<name>`

`telemetry.v1`: `utterances`, `transcripts`, `errors`, `audio` (s), `asr.time` (s), `chunks.skipped`, `bus.connected`.

## stdout

A line `{"heard": "...", "audio_ms": N, "asr_ms": N}` per transcript, then one report per interval:

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `recognizer` | string | the model, `synthetic`, `none` |
| `utterances`, `transcripts`, `empty`, `errors` | numbers | since start |
| `audio_s`, `asr_ms_mean`, `real_time_factor` | numbers | decode time over audio time |
| `noise_floor_dbfs`, `speaking` | number, bool | the segmenter's state |
| `last_text`, `last_error` | string/null | |
| `bus` | object | `connected`, `streaming`, `published`, `skipped_chunks`, `error` |

## Store vector (id 33)

Eight floats; a source that is absent is **-1**.

| Slot | Name | Unit |
|---|---|---|
| 0 | `transcripts` | count |
| 1 | `rtf` | decode time / audio time |
| 2 | `asr_ms_mean` | ms |
| 3 | `noise_floor_dbfs` | dBFS |
| 4 | `audio_s` | seconds heard |
| 5 | `skipped_chunks` | count |
| 6 | `bus_connected` | 0 or 1 |
| 7 | `errors` | count |

## Export (`http://127.0.0.1:8067`, continuous mode)

`/status`, `/raw` (latest raw view), `/history` (last 60 reports), `/guide`, `/healthz`.

## Settings

`--interval`, `--window`, `--source`, `--name`, `--lang`, `--margin-db`, `--min-db`, `--hang-ms`, `--min-ms`, `--max-ms`, `--threads`, `--python`, `--model-dir`, `--api-bind`, `--replay`, `--simulate`.
