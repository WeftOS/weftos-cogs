# Wiring

> 3.3 V logic only. Four wires to the Seed header, plus two straps that stay on the breakout.

```diagram
header
```

```diagram
wiring
```

The diagram uses the datasheet pin names, not a silkscreen this lane has read. If your board says VCC, SDA and SCL, those are the same nets as VDD, SDI and SCK. Trust the board's labels.

| Breakout | Seed header | Notes |
|---|---|---|
| VDD (board label may say VCC) | Pin 1, 3V3 | Not pin 2 or pin 4. See below. |
| GND | Pin 6, GND | Common ground |
| SDI (board label may say SDA) | Pin 3, GPIO2, SDA | I2C data |
| SCK (board label may say SCL) | Pin 5, GPIO3, SCL | I2C clock |
| SDO | Strap on the breakout | GND → address 0x76 (the default). VDDIO → 0x77. Do not leave it floating. |
| CSB | Strap to VDDIO (3.3 V) | Selects I2C. If CSB is low at power-up, the chip locks into SPI until the breakout's power is removed. |

SDO and CSB are not wired to Pi GPIO. They are straps on the breakout. This lane could not see how the purple board straps them.

## 3.3 V, not 5 V

Datasheet (vendor, not our measurement): VDD is 1.71 V to 3.6 V, VDDIO is 1.2 V to 3.6 V, and the absolute maximum on either supply pin is 4.25 V. Interface pins must stay within −0.3 V to VDDIO + 0.3 V. The Pi's GPIO is not 5 V tolerant. A 5 V header pin, or a breakout whose I2C pull-ups go to 5 V, can damage the Pi even if the sensor survives.

**Unverified:** whether this purple board has a regulator between the header and the chip. Until that is seen, power it only from pin 1 (3.3 V).

## Power-cycle the breakout, not just the Seed

A Seed reboot does not drop the 3.3 V pin, so the BME680 stays powered. Changing the SDO strap or repairing a CSB that was low at power-up does nothing until the breakout itself loses power. Unplug the breakout's VDD wire, or unplug the Seed's supply. Then plug it back.

I2C on the Seed (`dtparam=i2c_arm=on` and the `i2c-dev` module) is an owner approval. This cog does not edit `/boot`. If a reflash or OTA removes `/dev/i2c-1`, the cog reports that the open failed; it does not turn the bus back on.
