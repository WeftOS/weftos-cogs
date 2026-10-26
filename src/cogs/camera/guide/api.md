# API reference

> The reading, the report, the store vector, the export endpoints and the settings.

## Reading `video.frame-ref/<source>`

`video.frame-ref.v1`, class `reading`: `source_id`, `node_id`, `t_ns`, `seq`, `hash` (BLAKE3 of the JPEG, 64 lower-case hex), `width`, `height`, `pixel_format` (`mjpeg`), `ttl_ms`. The bytes are in the cog's data directory as `frames/<hash>.jpg` for `ttl_ms`.

## Reading `telemetry/<source>`

`telemetry.v1`: `frames.total`, `frames.rate` (fps), `frames.refs`, `capture.errors`, `frame.width`, `frame.height`, `bus.connected`.

## stdout report (one per interval)

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `source`, `device` | string | the source id; `v4l2:<node>`, `simulated`, `replay`, `none` |
| `frames`, `fps` | numbers | frames kept in the window and their rate |
| `width`, `height` | number/null | size of the last frame |
| `mean_frame_kb` | number | mean JPEG size |
| `frame_refs_published` | number | readings sent to the broker |
| `capture_errors` | number | unreadable frames |
| `last_hash` | string/null | hash of the newest frame |
| `bus` | object | `connected`, `seq`, `error` |
| `error` | string | only with `no_source` |

## Store vector (id 31)

Eight floats; a source that is absent is **-1**.

| Slot | Name | Unit |
|---|---|---|
| 0 | `fps` | frames per second |
| 1 | `width` | px |
| 2 | `height` | px |
| 3 | `mean_frame_kb` | KiB |
| 4 | `frames` | count in the window |
| 5 | `bus_connected` | 0 or 1 |
| 6 | `capture_errors` | count |
| 7 | `refs_published` | count |

## Export (`http://127.0.0.1:8065`, continuous mode)

`/status` latest report, `/raw` the latest frame handle, `/frame.jpg` the latest JPEG, `/history` the last 60 reports, `/guide` this guide, `/healthz`. Loopback only.

## Settings

`--interval`, `--fps`, `--width`, `--height`, `--ttl-ms`, `--source`, `--camera-name`, `--device`, `--frames-dir`, `--api-bind`, `--replay`, `--simulate`.
