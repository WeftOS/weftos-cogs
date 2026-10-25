# API reference

> Every field the cog reports, the store vector, the export endpoints, and the settings.

## stdout report (one per window)

| Field | Type | Meaning |
|---|---|---|
| `status` | string | `ok`, `no_source`, `simulate` |
| `provenance` | string | `MEASURED` or `SYNTHETIC` |
| `source` | string | `sysfs:<root>` or `simulated` |
| `sources` | object | booleans: `power`, `fan`, `thermal`, `load`, `mem` |
| `missing` | array | the names above that are false |
| `power_w`, `voltage_v`, `current_a` | number/null | window mean from the first INA2xx |
| `fan_rpm` | number/null | window mean of the fastest fan |
| `temp_max_c` | number/null | hottest zone reading in the window |
| `temp_by_group_c` | object | hottest per group: `cpu`, `gpu`, `npu`, `camera`, `ddr`, `other` (present groups only) |
| `load1`, `mem_available_mb` | number/null | |
| `power_monitors`, `fans`, `zones` | arrays | per-device detail from the last sample |
| `zones_unreadable` | int | zones skipped as unreadable or implausible |
| `vector_slots`, `vector` | arrays | the ingest vector and its slot names |
| `samples` | int | reads in the window |
| `error` | string | only with `no_source` |

## Store vector (id 28)

Eight floats in physical units. A source that is absent is **-1** (no real reading is negative, and JSON cannot carry NaN).

| Slot | Name | Unit |
|---|---|---|
| 0 | `power_w` | W |
| 1 | `voltage_v` | V |
| 2 | `current_a` | A |
| 3 | `temp_max_c` | C, hottest of all zones |
| 4 | `temp_npu_c` | C, hottest `nsp*` zone |
| 5 | `temp_cpu_c` | C, hottest `cpu*` zone |
| 6 | `fan_rpm` | RPM |
| 7 | `load1` | 1-minute load average |

## Export (`http://127.0.0.1:8062`, continuous mode)

| Endpoint | Returns |
|---|---|
| `GET /status` | the latest report |
| `GET /history` | `{"reports":[...]}`, the last 60 |
| `GET /guide` | this guide as JSON |
| `GET /healthz` | `{"ok":true}` |

## Settings (cog.toml / CLI)

| Setting | CLI | Default |
|---|---|---|
| interval | `--interval` | 5 s |
| window | `--window` | 2 s (`--once`) |
| sample_ms | `--sample-ms` | 1000 ms |
| sysfs_root | `--sysfs-root` | `/` |
| api bind | `--api-bind` | `127.0.0.1:8062` |
| simulate | `--simulate` | off |
