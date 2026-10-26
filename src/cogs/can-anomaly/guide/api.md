# API reference

> The readings, the report, the store vector, the export endpoints and the settings.

## Reading `event/<name>` (default `can-anomaly0`)

`event.v1`, class `reading`: `kind` (`can.baseline`, `can.new_id`, `can.dlc_change`, `can.id_missing`, `can.id_back`, `can.rate_high`, `can.rate_low`, `can.flood`), `severity` (`info`, `warn`, `alert`), `subject` (`0x1a4`, `0x18ff0001` for an extended id, `bus`), `detail`, and `value` when there is a number (a rate, a length code, the id count).

## Reading `telemetry/<name>`

`telemetry.v1`: `frames.seen`, `baseline.ids`, `events`, `monitoring`, `records.skipped`, `bus.connected`.

## stdout

One JSON line per event, then one report per interval:

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `phase` | string | `learning` or `monitoring` |
| `frames_seen`, `baseline_ids`, `baseline_frames_per_s` | numbers | |
| `events`, `events_by_kind` | number, object | not counting the baseline event |
| `last_events` | array | the five latest |
| `bus` | object | `connected`, `subscribed`, `published`, `skipped_records`, `error` |

## Store vector (id 39)

Eight floats; a source that is absent is **-1**.

| Slot | Name |
|---|---|
| 0 | `frames_seen` |
| 1 | `baseline_ids` |
| 2 | `events` |
| 3 | `new_ids` |
| 4 | `missing_ids` |
| 5 | `rate_events` (high plus low) |
| 6 | `floods` |
| 7 | `monitoring` (0 learning, 1 watching) |

## Export (`http://127.0.0.1:8073`, continuous mode)

`/status`, `/raw`, `/history`, `/guide`, `/healthz`.

## Settings

`--interval`, `--window`, `--source`, `--name`, `--learn-s`, `--window-s`, `--rate-high-x`, `--rate-low-frac`, `--cooldown-s`, `--api-bind`, `--replay`, `--simulate`.
