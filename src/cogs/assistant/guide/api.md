# API reference

> The readings, the report, the store vector, the export endpoints and the settings.

## Reading `transcript/<name>` (default `reply0`)

`transcript.v1`, class `reading`: the answer as `text`; `start_ns` and `end_ns` both the command's start time; `confidence` always 1.0 (not measured).

## Calls made

`svc.infer/<service>` with `infer.v1`: `model`, `priority`, `max_tokens`, `system`, `text`.

## Reading `telemetry/<name>`

`telemetry.v1`: `commands`, `replies`, `failures`, `latency.mean` (ms), `bus.connected`.

## stdout

`{"command", "reply", "ok"}` per command, then one report per interval:

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `model`, `brain` | string | the alias; `svc.infer/<service>` or `synthetic` |
| `commands`, `replies`, `failures` | numbers | since start |
| `mean_latency_ms` | number | per answered command, service time included |
| `remembered_turns` | number | |
| `last_command`, `last_reply`, `last_error` | string/null | |
| `bus` | object | `connected`, `subscribed`, `published`, `error` |

## Store vector (id 35)

Eight floats; a source that is absent is **-1**.

| Slot | Name |
|---|---|
| 0 | `commands` |
| 1 | `replies` |
| 2 | `failures` |
| 3 | `mean_latency_ms` |
| 4 | `remembered_turns` |
| 5 | `published` |
| 6 | `bus_connected` |
| 7 | `answer_ratio` |

## Export (`http://127.0.0.1:8069`, continuous mode)

`/status`, `/raw`, `/history`, `/guide`, `/healthz`.

## Settings

`--interval`, `--window`, `--model`, `--service`, `--source`, `--name`, `--max-tokens`, `--priority`, `--history`, `--timeout-s`, `--api-bind`, `--replay`, `--simulate`.
