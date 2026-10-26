# API reference

> The readings, the report, the store vector, the export endpoints and the settings.

## Reading `detections/<name>` (default `vis0`)

`detections.v1`, class `reading`: `source_id`, `node_id`, `t_ns` (the frame's), `seq`, `frame_hash` (BLAKE3 of the frame), `model`, `boxes` (at most 64): each `label`, `score` (0..1; 1.0 when the model gave none) and `bbox` `[x, y, w, h]` normalized to the frame.

## Call made

`svc.infer/<service>` with `infer.v1`: `model`, `priority`, `max_tokens`, `system`, `text`, `image_blob` (the frame's `blob.v1` handle).

## Reading `telemetry/<name>`

`telemetry.v1`: `frames.seen`, `frames.asked`, `detections`, `boxes`, `failures`, `latency.mean` (ms), `bus.connected`.

## stdout

`{"frame": "...", "labels": [...]}` per frame looked at, then one report per interval:

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `model`, `brain` | string | the alias; `svc.infer/<service>` or `synthetic` |
| `frames_seen`, `frames_asked`, `answers`, `detections_published` | numbers | since start |
| `boxes`, `labels_only_boxes` | numbers | boxes published; those that cover the whole frame |
| `unusable_frames`, `unparsed_answers`, `failures` | numbers | |
| `replaced_while_busy`, `throttled` | numbers | frames skipped for a newer one; frames skipped for the rate limit |
| `mean_latency_ms` | number | per answered frame |
| `last_labels`, `last_error` | array, string/null | |
| `bus` | object | `connected`, `subscribed`, `published`, `error` |

## Store vector (id 37)

Eight floats; a source that is absent is **-1**.

| Slot | Name |
|---|---|
| 0 | `frames_seen` |
| 1 | `frames_asked` |
| 2 | `published` |
| 3 | `boxes` |
| 4 | `mean_latency_ms` |
| 5 | `failures` |
| 6 | `replaced` |
| 7 | `bus_connected` |

## Export (`http://127.0.0.1:8071`, continuous mode)

`/status`, `/raw`, `/history`, `/guide`, `/healthz`.

## Settings

`--interval`, `--window`, `--model`, `--service`, `--source`, `--name`, `--min-interval-ms`, `--priority`, `--max-tokens`, `--timeout-s`, `--api-bind`, `--replay`, `--simulate`.
