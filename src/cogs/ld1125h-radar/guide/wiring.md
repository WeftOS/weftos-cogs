# Wiring

> Three wires. Module UTX to Pi pin 10. Leave URX unconnected so a getty cannot send commands. UART levels are 3.3 V.

Power the module from **pin 2 (5 V)**. The manual allows 3.3–5 V and the vendor debug diagram uses 5 V. About 80 mA typical is why pin 1 (3.3 V) is left alone. Never put 5 V on Pi GPIO.

## Find pin 1

```diagram
header
```

Hold the Seed with the 40-pin header along the top edge and the ports along the bottom, micro-SD at the left. Pin 1 (3.3 V) is the left pin of the inner row. The outer row is the even pins.

## The three wires

```diagram
wiring
```

| Module | Silk | Seed | Why |
|---|---|---|---|
| VCC, pin 1 | `5V` | pin 2 (5 V) | supply |
| GND, pin 2 | `G` | pin 6 (GND) | common ground |
| UTX, pin 4 | `UT` | pin 10 (GPIO15 RXD) | module talks, Pi listens |
| URX, pin 3 | `UR` | not connected | Pi pin 8 is TX. A console can write `rmax`, `save`, or `test_mode` |

`/dev/serial0` is the Seed console until the owner frees it. Prefer a 3.3 V USB-serial adapter on `/dev/ttyUSB0`: adapter RX to module UTX, adapter ground to module ground, and do not connect adapter TX. The cog's default device is `/dev/ttyUSB0`.

The cog opens the tty read-write only so the baud can be set. It has no write path and does not send those commands.

## Pins to leave alone

| Pin | Why |
|---|---|
| 1 | 3.3 V rail |
| 4 | the other 5 V pin |
| 8 | GPIO14 TXD, the console transmit pin |
