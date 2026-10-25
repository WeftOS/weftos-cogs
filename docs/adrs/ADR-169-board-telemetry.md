# ADR-169: board-telemetry, board health from sysfs

**Status:** Proposed
**Date:** 2026-10-25
**Cog:** `board-telemetry` 0.1.0

## Context

Placement needs live facts about the board a cog runs on: how much power it draws, how hot it is, whether the fan turns. The Ventuno Q cog plan (section 5.4) calls for a `board-telemetry` cog over an INA232 power monitor, an EMC2305 fan controller and the SoC thermal zones, to feed a power budget and thermal headroom. A Raspberry Pi has thermal and load data but no power monitor or fan controller by default. One cog has to serve both without knowing which it is on.

The kernel already publishes all of this through hwmon and thermal sysfs. hwmon and zone numbers are assigned at probe time and differ between boards, so nothing can be addressed by number.

## Decision

1. **Discover by name, never by number.** `src/sources.rs` scans `/sys/class/hwmon/hwmon*` (sorted numerically) and `/sys/class/thermal/thermal_zone*`.
   - A hwmon whose `name` starts `ina2` (ina219, ina226, ina232, ina238, ina2xx) is a power monitor: `power1_input` (uW), `in1_input` (mV, bus voltage), `curr1_input` (mA). If `power1_input` is absent and V and I are present, power is V x I and `power_derived` is set.
   - Any hwmon with `fanN_input` contributes fans. The headline is the fastest fan; every fan is listed.
   - Each thermal zone gives `type` and `temp` (m degC). The group comes from the `type` prefix: `cpu*`, `gpu*`, `nsp*` (NPU), `camss*` (camera), `ddr*`, else other. A zone that cannot be read, or reads outside -50..150 C, is counted in `zones_unreadable` and left out of every maximum.
   - `/proc/loadavg` (first field) and `/proc/meminfo` `MemAvailable`.
2. **`--sysfs-root PATH`** replaces `/` as the directory holding `sys/` and `proc/`. Tests run against fixture trees in `tests/fixtures/` (`ventuno`, `pi`, `empty`).
3. **Window.** Samples are taken `--sample-ms` apart (default 1000) for `--interval` seconds, or `--window` seconds with `--once` (default 2). Power, voltage, current, fan speed and load are the window mean. Temperatures are the window maximum, overall and per group.
4. **Ingest vector, id 28** (after 21 to 27 used by the other sensor cogs), posted with `dedup: true` as the other cogs do, to `$COGNITUM_INGEST_URL` when the node sets it (the bridge may be on a loopback fallback port; the URL must be `http://` on a loopback host, otherwise it is ignored with a warning) and to `127.0.0.1:80/api/v1/store/ingest` when it is not. `$COGNITUM_COG_TOKEN` is sent as a bearer token when present (`src/ingest.rs`). Eight floats in physical units:

   | Slot | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
   |---|---|---|---|---|---|---|---|---|
   | Name | power_w | voltage_v | current_a | temp_max_c | temp_npu_c | temp_cpu_c | fan_rpm | load1 |

   An absent source is **-1.0**. JSON cannot carry NaN, no real reading here is negative, and the report's own fields are `null`. Values are rounded to 3 places. The headline power comes from the first INA2xx by hwmon number; the others are listed in `power_monitors`.
5. **Fail honest.** `status` is `no_source` when no INA2xx, fan or thermal zone is readable. `/proc` alone does not count, so a container with a stripped `/sys` does not look healthy. The report then has `error`, null readings and `vector` of -1, nothing is posted, and `--once` exits 0. In continuous mode the cog keeps reporting `no_source` and recovers if a source appears. `missing` names every absent source on an `ok` report, so a Pi says `["power","fan"]`.
6. **`--simulate`** returns Ventuno-shaped synthetic values with `status` `simulate` and `provenance` `SYNTHETIC`. It never posts to the store, which differs from `sound-detect`: a synthetic power draw in a placement input is worse than a gap.
7. **Export.** `[api] bind_port = 8062`, loopback only, continuous mode only: `/status`, `/history` (last 60 reports), `/guide`, `/healthz`, CORS `*`. `--once` does not bind a port.
8. **Guide** (ADR-104): `guide/guide.toml` with pages `start`, `sources`, `setup`, `troubleshoot`, `api`, compiled in and served at `/guide`. There is no header or wiring section; the cog opens no bus.
9. `cog.toml` carries a comment where `interfaces = ["none"]` belongs. The field is not declared until the WeftOS manifest accepts it. `hardware_requirement` is `v0-appliance`, the id `cog_lint.py` accepts for a Linux board.

## Consequences

- No hardware is opened. The cog needs only read access to `/sys` and `/proc`.
- Only the first INA2xx drives the headline. A board with several rails needs a selector flag; none exists yet.
- The NPU temperature is the `nsp*` zone. A board that names its NPU zone otherwise shows it under `other` until the prefix list grows.
- A stopped fan reports 0 RPM, which is a reading. A fan with no `fanN_input` is absent.
- A bare `--once` is listed in the console commands. On a Linux host that has thermal zones, `scripts/smoke_cog.py` expects `no_source` from it and fails; run the gate on a host without them or in a container.
- NPU queue depth (named in the plan row) is not read: no stable sysfs source exists to read it by name. Deferred.

## Verification

Unit tests pin the parsers, name discovery with hwmon3 and hwmon10, the grouping, the window folding and the vector layout. `tests/cli.rs` runs the binary against the three fixtures and `--simulate`. No on-device run has been done.
