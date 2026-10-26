# API reference

> The service, the report, the store vector, the export endpoints and the settings.

## Service `svc.mcu/<service>`

Request `mcu.call.v1`:

| Field | Type | Rule |
|---|---|---|
| `method` | string | `[a-z][a-z0-9_.]*`, at most 64 chars, on the allowlist |
| `params` | array | numbers, booleans and strings, at most 8; default empty |

Reply body `{"result": <value>, "ms": <round trip in milliseconds>}`. A refusal or a router error reaches the caller as `service_error` with the message. The deadline is the call's `timeout_ms`, capped at 5 s for the router.

## Reading `telemetry/<service>`

`telemetry.v1`: `mcu.ok`, `ping.latency` (ms), `sketch.calls`, `calls.served`, `calls.failed`, `calls.refused`, `bus.serving`.

## stdout report (one per interval)

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `foreign_sketch`, `no_source`, `simulate`, `replay` |
| `provenance` | string | `MEASURED`, `SYNTHETIC` or `REPLAY` |
| `router` | string | `router:<socket>`, `simulated`, `replay` |
| `mcu` | string | `ok`, `foreign-sketch`, `no_router`, `no_answer` |
| `ping_ms`, `sketch_calls` | number/null | the probe's latency and the sketch's `weftos_calls` counter |
| `service` | string | `svc.mcu/<service>` |
| `allowed_methods` | array | |
| `calls_served`, `calls_failed`, `calls_refused` | numbers | since start |
| `bus` | object | `connected`, `serving`, `error` |
| `error` | string | with `no_source` and `foreign_sketch` |

## Store vector (id 32)

Eight floats; a source that is absent is **-1**.

| Slot | Name | Unit |
|---|---|---|
| 0 | `ping_ms` | ms |
| 1 | `sketch_calls` | the sketch's counter |
| 2 | `calls_served` | count |
| 3 | `calls_failed` | count |
| 4 | `calls_refused` | count |
| 5 | `mcu_ok` | 0 or 1 |
| 6 | `serving` | 0 or 1 |
| 7 | `router_present` | 1 when the router answered |

## Export (`http://127.0.0.1:8066`, continuous mode)

`/status` latest report, `/raw` the latest probe, `/history` the last 60 reports, `/guide` this guide, `/healthz`.

## Settings

`--interval`, `--service`, `--allow`, `--socket`, `--api-bind`, `--replay`, `--simulate`.
