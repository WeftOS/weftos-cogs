# ld1125h-radar

A Cognitum cog for the **Hi-Link HLK-LD1125H-24G** 24 GHz FMCW human-presence radar. It reads 3.3 V UART at 115200 8N1 and accepts ASCII lines `mov, dis=<metres>` and `occ, dis=<metres>`. It does not send configuration.

Breathing micro-motion is how the module detects a still person. This cog does not report a heart rate, a breathing rate, a track, or a person. ADR-172.

The sensor guide is `guide/` (WeftOS ADR-104), served at `GET /guide` on `127.0.0.1:8054`.

## Wiring

| Module | Silk | Seed |
|---|---|---|
| VCC | `5V` | pin 2 (5 V) |
| GND | `G` | pin 6 (GND) |
| UTX | `UT` | pin 10 (GPIO15 RXD) |
| URX | `UR` | leave open |

`/dev/serial0` is the console until the owner frees it. Prefer a 3.3 V USB-serial adapter on `/dev/ttyUSB0`. Do not connect Pi pin 8 (TX) to URX. UART data is 3.3 V. Never put 5 V on a GPIO pin.

## Run

```bash
cog-ld1125h-radar --help
cog-ld1125h-radar --once --simulate
cog-ld1125h-radar --once --device /dev/ttyUSB0
```

`--once` with a missing device prints one `no_source` line (`kind` and `distance_m` null) and exits 0.

A build with `--features spatial-evidence` can write `spatial.evidence.v1` lines of type `radar_range` when `--spatial-out` is set and a pose and region are given. The default build has no spatial flags. No pose means no evidence line. This cog does not emit SenML.

## Tested

Host unit tests and `--simulate` only. **Hardware is not yet tested.** Datasheet numbers are the vendor's, not measurements.
