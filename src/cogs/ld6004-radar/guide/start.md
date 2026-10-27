# Start here

> Read an HLK-LD6004 3D presence radar on a Cognitum Seed. Positions stay in the radar's own frame until you set a pose.

This guide is for the Hi-Link HLK-LD6004, a 60 GHz presence radar. The `ld6004-radar` cog owns the UART: it finds frame boundaries, checks the checksum, and prints the values the LD6004 protocol defines. It does not report heart rate or breathing rate. Those rates are not in this module's protocol.

The module streams on its own. The default command sends nothing.

The axis convention below was checked on a bench module with the UART viewer on 2026-10-10. This cog binary has not yet been the process on that port. A `--simulate` run is not that check. Vendor current and range figures stay the vendor's.

## The short path

1. **Get the parts.** The module, a USB-serial adapter (or the Seed UART), and a 3.3 V supply that can deliver the vendor's current. See **Parts**.
2. **Wire it.** Module TX0 goes to the host RX. Do not use a 5 V pin. See **Wiring**.
3. **Leave the serial console alone.** `/dev/serial0` on a Seed is the console until the owner frees it. This cog does not change boot configuration. A USB-serial device at `/dev/ttyUSB0` avoids that.
4. **Run one window with no claim that the sensor was read.** `cog-ld6004-radar --once --simulate` prints a labelled simulated line.
5. **Point it at the real device.** `cog-ld6004-radar --once --device /dev/ttyUSB0`
6. **Confirm the firmware project** once, with `--command query-firmware`. Project codes 0 and 6 are the presence protocol this cog decodes. The bench module reported 6. See **Sensor**.

## How the bytes move

```diagram
flow
```

## What you get

| Output | Where | What it is |
|---|---|---|
| One JSON report per interval | stdout | Targets, point cloud, zones, module status. `vitals` is `not_defined`. |
| SenML pack | the next stdout line, and `GET /senml` | Module settings that arrived. Omitted when the window had none. |
| Loopback export | `http://127.0.0.1:8055` | `GET /status`, `/senml`, `/guide`. |
| Room tracks | only with the `spatial-evidence` feature and `--spatial-out` | `radar_track_point` lines. No pose means no line. |

The export is GET only. There is no PUT.
