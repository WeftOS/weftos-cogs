# Set up

> Install the binary, point it at a tty under `/dev`, and read one JSON line. Export stays on `127.0.0.1:8054`.

Replace `<seed>` with the appliance address. Do not put a real address, token, or tailnet name in this guide.

## No hardware

```bash
cog-ld1125h-radar --once --simulate
```

The simulator emits `mov, dis=1.25` and `occ, dis=0.75` through the real parser. The report has `"simulated": true`. That is not a measurement.

## One read, missing device

```bash
cog-ld1125h-radar --once --device /dev/ttyUSB0
```

If the device cannot be opened, the process prints one `no_source` line and exits 0. `kind` and `distance_m` are null. The error names the device, the baud, and the next wiring check.

## Continuous

```bash
cog-ld1125h-radar --device /dev/ttyUSB0 --interval 1
```

`--interval` is seconds between reports (1–60, default 1). `--window` is the `--once` capture length (1–30, default 2). `--baud` defaults to 115200. The manual documents that rate. Another rate is an operator override, not a documented module setting.

A device path must be under `/dev/`, at most 128 characters, with no `..`. Anything else exits 2 and prints nothing on stdout.

## On a Seed

```bash
# after the arm binary is installed on the appliance
curl -X POST http://<seed>/api/v1/apps/ld1125h-radar/console \
  -d '{"command":"--once --simulate"}'
```

Console commands the manifest allows: `--help`, `--once`, `--once --simulate`.

The export binds `127.0.0.1:8054` before the sensor is opened. If that port is taken, export is disabled and the stdout report continues. There is no flag to bind `0.0.0.0`.

## Spatial evidence

The default build has no spatial flags. A build with `--features spatial-evidence` still emits nothing until `--spatial-out` is a file path or `export`.

```bash
cog-ld1125h-radar --device /dev/ttyUSB0 \
  --spatial-out /var/lib/ld1125h/evidence.jsonl \
  --radar-pose 0.1,1.83,1.0,0 \
  --spatial-region region/urth/meso/room
```

No pose writes no evidence line and sets `spatial` to `no_pose`. No region sets `no_region`. `--spatial-out export` with `--once` exits 2. `source_id` defaults to `ld1125h-1` and is the installed object, not a person and not the catalog type.

Accepted lines become `radar_range` only. Presence text is not `human_confirm` and not a track. This cog does not emit SenML: the manual's distance is spatial, and the manual does not define a separate non-spatial quantity.
