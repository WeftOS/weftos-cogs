# Set up

> Run the cog on the board, check it, and see where the numbers go.

## Check by hand first

```sh
for d in /sys/class/hwmon/hwmon*; do echo "$d $(cat $d/name)"; done
for z in /sys/class/thermal/thermal_zone*; do echo "$z $(cat $z/type) $(cat $z/temp)"; done
```

An INA2xx shows up as `ina2xx`, `ina226`, `ina232` and so on. If the board has the part but no entry appears, the device-tree overlay or the kernel driver is not enabled; that is a board change, not a cog setting.

## Run it

```sh
cog-board-telemetry --once                   # 2 s window, one report
cog-board-telemetry --once --simulate        # synthetic values
cog-board-telemetry --interval 5             # continuous
cog-board-telemetry --once --sysfs-root DIR  # DIR must hold sys/ and proc/
```

## Outputs

| Output | Where |
|---|---|
| JSON report per window | stdout |
| 8-float vector, id 28 | `POST 127.0.0.1:80/api/v1/store/ingest`, only when `status` is `ok` |
| Export | `http://127.0.0.1:8062` (`/status`, `/history`, `/guide`, `/healthz`), continuous mode only |

`--simulate` never posts to the store.
