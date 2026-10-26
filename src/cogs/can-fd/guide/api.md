# API reference

> The stream, the report, the store vector, the export endpoints and the settings.

## Stream `can.frame/<source>`

`can.frame.v1`, class `stream`. Each record is the JSON of one data frame, at most 512 bytes: `source_id`, `node_id`, `t_ns` (the wall clock when it was read), `seq`, `id` (11 or 29 bits), `ext`, `fd`, `dlc` (0 to 15), `data` (lower-case hex).

## Reading `telemetry/<source>`

`telemetry.v1`: `frames.rx`, `frames.errors`, `bus.off`, `bus.load` (%), `rx.err_counter`, `tx.err_counter`, `rx.dropped`, `stream.seq`.

## stdout report (one per interval)

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `iface`, `stream` | string | |
| `frames`, `frames_per_s`, `fd_frames`, `extended_frames` | numbers | in the window |
| `remote_frames_skipped` | number | remote frames seen, not published |
| `bus_load_pct` | number | estimate, see **The bus** |
| `error_frames`, `error_classes`, `bus_off_events` | numbers, object, number | |
| `top_ids` | array | the five busiest ids with their frame counts |
| `controller` | object/null | `state`, `oper`, `bitrate`, `data_bitrate`, `fd`, `listen_only`, `loopback`, `virtual`, error counters |
| `configured_bitrate`, `bitrate_matches` | number, bool/null | null for a virtual interface |
| `kernel_rx_dropped` | number | the interface's `rx_dropped` since start |
| `transmit` | string | always `never` |
| `bus` | object | `connected`, `topic`, `seq`, `write_errors`, `error` |
| `error` | string | only with `no_source` |

## Store vector (id 38)

Eight floats; a source that is absent is **-1**.

| Slot | Name |
|---|---|
| 0 | `frames_per_s` |
| 1 | `bus_load_pct` |
| 2 | `error_frames` |
| 3 | `bus_off` |
| 4 | `rx_error_counter` |
| 5 | `tx_error_counter` |
| 6 | `kernel_rx_dropped` |
| 7 | `bitrate_ok` (1 matches, 0 differs, -1 no controller) |

## Export (`http://127.0.0.1:8072`, continuous mode)

`/status`, `/raw`, `/history`, `/guide`, `/healthz`.

## Settings

`--interval`, `--window`, `--iface`, `--bitrate`, `--source`, `--api-bind`, `--replay`, `--simulate`.
