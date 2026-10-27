# Wiring

> Module TX0 goes to Seed pin 10 (RXD). Module RX0 goes to Seed pin 8 (TXD) only if you will send the firmware query. Ground is shared. 5 V stays disconnected.

```diagram
header
```

```diagram
wiring
```

| Module pin | Name | Connect to |
|---|---|---|
| 1 | OUT | Leave open. It is a presence GPIO, not UART. |
| 2 | TX0 | Seed pin 10, RXD. This is the wire a read-only cog needs. |
| 3 | RX0 | Seed pin 8, TXD. Only for `--command query-firmware`. |
| 4 | GND | Seed pin 6, GND, and the regulator ground. |
| 5 | 3V3 | A 3.3 V supply, 3.1 V to 3.5 V. See **Parts** for the vendor current. |

Seed pins 2 and 4 are 5 V. Do not connect them to the module or to the signal wires. The Pi's GPIO is not 5 V tolerant.

A read-only run needs module TX0, ground, and power. The host does not have to drive RX0 until you ask for the firmware query.

Cross the data wires. TX on the module is RX on the Seed. Two TX pins tied together will not pass a frame.

The baud in the datasheet is 115200 with no parity. The datasheet does not state the word length or the stop bit. The cog opens 8 data bits and 1 stop bit. That is an assumption, recorded in the ADR. Do not send a baud-change command. The printed baud table includes a value this cog does not treat as a real rate.

`/dev/serial0` on a Seed is the mini UART, and the serial console owns it until the owner changes boot configuration. This cog will not do that. Prefer a USB-serial adapter.

After a wiring change, remove power from the radar itself. Restarting the cog does not reset the module.

The diagram shows Seed pin 1 beside the module's 3V3 pin so the header drawing has a 3.3 V net. If the module draws the vendor's 1 A, power it from the external regulator and keep pin 1 unused. Ground still has to be common.

## ESP32-S3

Use the silkscreen labels **1** and **2** on an ESP32-S3-DevKitC-1. Those are GPIO1 and GPIO2, on header J3 at positions 4 and 5. They are not strapping pins on the S3. Header position 1 on that side is GND. Header position 2 is GPIO43, the console TX. Do not put the radar on the console UART (GPIO43 TX / GPIO44 RX).

| ESP32-S3 | Radar | Wire |
|---|---|---|
| GPIO1, silkscreen `1` | TX0 | Radar TX into the ESP RX |
| GPIO2, silkscreen `2` | RX0 | ESP TX into the radar RX |
| GND (J3 position 1, or any GND) | GND | Common ground, including the radar supply |
| Do not use the DevKit 3V3 pins | 3V3 | A separate 3.3 V supply, above 1 A |

Open a UART on GPIO2 as TX and GPIO1 as RX, 115200 8N1. Tie the radar BOOT1 pin to GND before power-up. Leave SWCLK and SWDIO open. Leave OUT open for this UART path.

The DevKit's own 3V3 pins power the ESP module. They are not the radar supply. The radar can draw hundreds of milliamps. Share ground between that supply and the ESP. Keep 5 V off the radar and off the UART wires.
