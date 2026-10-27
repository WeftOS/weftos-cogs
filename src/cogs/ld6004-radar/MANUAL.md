# HLK-LD6004 manual

Operator notes for the `ld6004-radar` cog and the `weft-ld6004-scope` UART viewer. The paged guide under `guide/` is the Seed guide. This file is the bench manual. Vendor PDFs stay out of the repo. Protocol: Hi-Link, HLK-LD6004 module communication protocol, cover date September 2025. Datasheet V1.1.

## What this is

A 60 GHz presence radar (ADT6101P, about 57–64 GHz). UART 115200 8N1, TinyFrame, start byte `0x01`. It streams on its own. It does not report heart rate or breathing rate. `0x0A14` is the unattended low-power state, not a rate.

The cog's default command sends nothing. The only cog command is `--command query-firmware`, which sends the 8-byte identity query once. The UART viewer can send other commands when you click them. Do not point an LD6002 decoder at this module.

## Module header

Hold the board so `HLK-LD6004` reads on the left and `V1.0` on the right.

Right row, top to bottom: 3V3, GND, RX, TX, OUT.

Left row, top to bottom: GND, SWCLK, SWDIO, BOOT1, 3V3.

| Pin | Use |
|---|---|
| TX | Module transmit. Connect to the host RX. |
| RX | Module receive. Connect to the host TX. Needed for a query or a viewer command. |
| GND | Common with the host and with the supply. |
| 3V3 | 3.1–3.5 V from a supply that can deliver more than 1 A. Typical draw is about 135 mA, and the vendor max is 600 mA. |
| OUT | Presence GPIO. Leave open for UART. |
| BOOT1 | Tie to GND before power-up. An open BOOT1 can keep the application from starting. It is not a flash-mode strap. |
| SWCLK, SWDIO | Leave open while you listen on UART. |

Do not power the module from a USB-UART 3V3 pin or from an ESP32 3V3 pin. Set a USB-UART adapter to 3.3 V I/O. Do not connect 5 V.

## ESP32-S3-DevKitC-1

Pins **1** and **2** mean the silkscreen labels, which are GPIO1 and GPIO2.

On header J3 (the side marked TX and RX):

| J3 position | Silkscreen | GPIO | Use for this radar |
|---|---|---|---|
| 1 | G | GND | Common ground |
| 2 | TX | GPIO43 | Console TX. Do not use. |
| 3 | RX | GPIO44 | Console RX. Do not use. |
| 4 | 1 | GPIO1 | ESP RX. Radar TX lands here. |
| 5 | 2 | GPIO2 | ESP TX. This wire goes to radar RX. |

J1 positions 1 and 2 are both 3V3. Those are not the signal pair.

Wire, in order:

1. Radar TX → ESP GPIO1 (label `1`).
2. Radar RX → ESP GPIO2 (label `2`).
3. Radar 3V3 → a separate 3.3 V supply, above 1 A. Not the DevKit 3V3 pin.
4. Radar GND, supply GND, and ESP GND tied together.
5. Radar BOOT1 → GND, before you apply 3.3 V.

UART settings: 115200, 8 data bits, no parity, 1 stop bit. On the ESP, the UART TX pin is GPIO2 and the RX pin is GPIO1. GPIO1 and GPIO2 are not strapping pins on the ESP32-S3 (the straps are GPIO0, GPIO3, GPIO45, and GPIO46). Classic ESP32 GPIO1 is the console TX. That is a different chip.

## USB-UART bench

Cross the wires the same way: module TX to adapter RX, module RX to adapter TX, common GND, radar power from the separate 3.3 V supply. The bench adapter on this Mac was `/dev/cu.usbserial-214110`. Another host is often `/dev/ttyUSB0`.

The viewer:

```text
cargo run -p cog-ld6004-scope --bin weft-ld6004-scope -- --port /dev/cu.usbserial-214110
```

`--probe` prints one text sample and exits. The window plots forward (Z) and a front view where up is −Y. A blue line marks 0.30 m, the camera height used on the bench. Click a control to send it. The cog never sends those controls.

Stop any other process that already has the port open before you start the viewer or the cog.

## Frame

Stdout `frame` is `radar_local`.

- Z forward, out of the antenna.
- X lateral.
- above_radar_m = −Y.
- camera_above_radar_m = 0.30. Report only. Not removed from X.

Room tracks need a build with `--features spatial-evidence`, plus `--spatial-out`, `--radar-pose`, and `--spatial-region`. No pose means no evidence line. One cluster id is a track, not a person. Export stays on `127.0.0.1:8055`.

## Cog commands

```text
cargo run -- --once --simulate
cargo run -- --once --device /dev/ttyUSB0
cargo run -- --once --device /dev/ttyUSB0 --command query-firmware
```

`--once` with no device prints one `no_source` line and exits 0. `--simulate` is labelled simulated. Project codes other than 0 and 6 set `foreign_firmware` and drop presence fields.

## Not in this cog

No install write, no baud change, no OTA (`0x3000`), no point-cloud toggle, and no destructive setters. Those stay on the viewer, one click at a time, if you choose to send them. The cog does not free the Seed serial console. `/dev/serial0` stays the console until the owner changes boot configuration.
