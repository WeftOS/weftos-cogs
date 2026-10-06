# API

> A status object, then an optional SenML pack. The HTTP export is GET-only and bound to 127.0.0.1:8057.

## Stdout

Line 1 is always one JSON object:

| Field | Meaning |
|---|---|
| `schema` | `weavelogic.cog-output.v0` |
| `health` | `ok`, `degraded`, or `no_source` |
| `timestamp` | Unix seconds |
| `error` | `null`, or why a quantity or the sample is missing |
| `source.bus` | `/dev/i2c-1` unless you set another bus |
| `source.address` | `0x76` or `0x77` |
| `source.source_id` | The `--source-id` string, default `bme680/` |
| `source.simulated` | `true` only for `--simulate` |
| `source.verified` | Always `false` until hardware is measured |
| `temperature_c`, `humidity_rh`, `pressure_pa`, `gas_ohm` | Numbers, or `null`. Never a stand-in zero. |

Line 2, only when at least one quantity exists and `--readings-out` is not set, is a SenML pack: a JSON array.

| Record | `n` | `u` |
|---|---|---|
| Temperature | `temperature` | `Cel` |
| Humidity | `humidity` | `%RH` |
| Pressure | `pressure` | `Pa` |
| Gas resistance | `gas_resistance` | `Ohm` |

`bn` on the first record is `--source-id` and ends with `/`. `proof_` is on every record (`MEASURED` or `SYNTHETIC`) because it does not propagate. `t` is Unix seconds on every record. There is no `bt` and no `bu`. A missing quantity is left out of the array.

`gas_resistance` is not in the short example vocabulary in RuView ADR-384. Unknown names are stored and counted. They are not people, tracks, or bounding boxes.

`--readings-out PATH` writes that array to the file instead of stdout. The status line is still printed. A failed sample does not create the file and does not write zeros.

## HTTP

Bound to `127.0.0.1:8057` unless `--api-bind` sets another port on `127.0.0.1`. Other hosts are refused.

| Method and path | Body |
|---|---|
| `GET /` | A short index |
| `GET /status` | The latest status object |
| `GET /readings` | The latest SenML array, or a JSON object with `"readings": null` when there is no pack |
| `GET /guide` | `{"toml", "pages"}` for this cog version |

`GET /readings` is never `[]`. An empty array would look like a successful pack with no records.

The cog does not implement `PUT`. If the Seed's app API accepts `PUT /api/v1/apps/bme680/config`, that call replaces the whole config. Send every key.

There is no store vector. These four numbers are not an embedding, and they are not ingested as one.

## Flags

`--once`, `--simulate`, `--interval`, `--i2c-bus`, `--i2c-addr`, `--device`, `--source-id`, `--heater-c`, `--heat-ms`, `--api-bind`, `--readings-out`, `--help`.

An unknown flag, including `--spatial-out`, exits 2 with empty stdout. This binary has no spatial-evidence feature.
