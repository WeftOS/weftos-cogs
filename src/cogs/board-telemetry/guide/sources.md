# What it reads

> Where each number comes from, and how the cog finds it without hard-coded device numbers.

Everything is found by name. hwmon and thermal zone numbers change between boards and between boots, so the cog scans `/sys/class/hwmon/hwmon*` and `/sys/class/thermal/thermal_zone*` and decides from the contents.

| Number | Source | Unit conversion |
|---|---|---|
| `power_w` | hwmon whose `name` starts `ina2` (ina219, ina226, ina232, ina2xx): `power1_input` | microwatts to W |
| `voltage_v` | the same hwmon: `in1_input` (bus voltage) | millivolts to V |
| `current_a` | the same hwmon: `curr1_input` | milliamps to A |
| `fan_rpm` | any hwmon with `fanN_input` (for example emc2305, pwm-fan); the fastest fan | RPM as is |
| `temp_max_c` | every `thermal_zone*/temp` | millidegrees to degrees C |
| `temp_by_group_c` | the same zones, grouped by `type` | |
| `load1` | `/proc/loadavg`, first field | |
| `mem_available_mb` | `/proc/meminfo` `MemAvailable` | kB to MB |

If a driver exposes no `power1_input` but does give `in1_input` and `curr1_input`, the cog computes power as V times A and sets `power_derived` on that monitor.

## Thermal groups

A zone's `type` decides its group by prefix: `cpu*` is cpu, `gpu*` is gpu, `nsp*` is npu (the Qualcomm NPU on a Ventuno Q), `camss*` is camera, `ddr*` is ddr, anything else is other. Only groups that have a zone appear in `temp_by_group_c`.

A zone whose temperature cannot be read, or reads outside -50 to 150 C, is not counted as a temperature. It is counted in `zones_unreadable`.

## Reading `missing`

`missing` lists the sources this board does not have: any of `power`, `fan`, `thermal`, `load`, `mem`. The matching numbers are `null` in the report and `-1` in the vector.
