# API reference

> The readings, the report, the store vector, the export endpoints and the settings.

## Reading `transcript/<name>` (default `wake0`)

`transcript.v1`, class `reading`: the command as `text`, with the times of the transcript it came from. `confidence` is always 1.0 (not measured).

## Reading `telemetry/<name>`

`telemetry.v1`: `transcripts.seen`, `commands`, `wakes.bare`, `transcripts.ignored`, `bus.connected`.

## stdout

`{"command": "...", "from": "..."}` per command, then one report per interval:

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `phrase`, `listening_to` | string | |
| `transcripts_seen`, `commands`, `bare_wakes`, `ignored` | numbers | since start |
| `armed` | bool | a bare phrase is waiting for its command |
| `last_command` | string/null | |
| `bus` | object | `connected`, `subscribed`, `published`, `error` |
| `error` | string | only with `no_source` |

## Store vector (id 34)

Eight floats; a source that is absent is **-1**.

| Slot | Name |
|---|---|
| 0 | `seen` |
| 1 | `commands` |
| 2 | `bare_wakes` |
| 3 | `ignored` |
| 4 | `armed` (0 or 1) |
| 5 | `published` |
| 6 | `bus_connected` (0 or 1) |
| 7 | `command_ratio` (commands / seen) |

## Export (`http://127.0.0.1:8068`, continuous mode)

`/status`, `/raw`, `/history`, `/guide`, `/healthz`.

## Settings

`--interval`, `--window`, `--phrase`, `--listen-s`, `--source`, `--name`, `--api-bind`, `--replay`, `--simulate`.
