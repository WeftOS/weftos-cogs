# API

> Stdout is one report object, then an optional SenML array. The export is GET-only on loopback.

## Report

`schema` is `weavelogic.cog-output.v0`. `cog` is `ld6004-radar`. `timestamp` is Unix seconds. `sensor_class` is `radar`.

`source.kind` is `ld6004-uart`. `source.verified` is true only when checksum-valid frames arrived and the run was not `--simulate`. `source.simulated` is true for `--simulate`.

`health` is `ok`, `degraded`, or `no_source`. `quality` is null when `health` is `no_source`.

`frame` is `radar_local`. `axes` names that frame: X lateral, Y toward the bottom edge of the board, Z forward out of the antenna. `camera_above_radar_m` is 0.30, the bench camera height. It is not subtracted from X. `targets` is the `0x0A04` list for this window (empty means none arrived). Each item has `x_m`, `y_m`, `z_m`, `above_radar_m` (`-y_m`), `dop_idx`, `cluster_id`. There is no `velocity_m_s`. `dop_idx` is the protocol's integer; its unit is not stated. `points` is the `0x0A08` list, with the same height field. `zones` is null until a zone frame arrives, then four values of true, false, or null.

`vitals` is the string `not_defined`. The JSON has no heart-rate field and no breathing-rate field.

`writes` is `none` or `query-firmware`. `foreign_firmware` is true after a project code other than 0 or 6.

## SenML

When the window contains module status, the next stdout line is one JSON array. Every record has `bn` (the source id with a trailing slash), `n`, `v`, and `proof_`. `proof_` is `MEASURED` for UART bytes and `SYNTHETIC` for `--simulate`.

`hold_delay` uses SenML unit `s`. `z_min` and `z_max` use `m`. `sleep_period` is milliseconds and has no SenML unit, so `u` is omitted. `stay_life`, `output_interval`, and the enum fields also omit `u`.

Person positions and zone occupancy are not SenML.

## Export

Bound to `127.0.0.1:8055` by default. `GET` only.

| Path | Body |
|---|---|
| `/` | A short index |
| `/status` | The latest report |
| `/senml` | The latest pack, or `[]` |
| `/guide` | `{"toml","pages"}` for this cog version |
| `/spatial` | NDJSON, only in a `spatial-evidence` build, and only for lines produced with `--spatial-out export` |

A browser on another machine cannot open the export. A program on the same Seed can.

## Spatial line

`schema` is `spatial.evidence.v1`. `type` is `radar_track_point`. `frame` is `room_enu`. `proof` is `MEASURED` or `SYNTHETIC`. `source_id` is the installed id without a trailing slash. The line is not written to stdout. It goes to the file, or to `GET /spatial`.

No other ADR-384 type is emitted.
