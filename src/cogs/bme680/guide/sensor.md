# The sensor

> A Bosch Sensortec BME680: gas, pressure, humidity and temperature in one 3.0 mm package, read over I2C.

The BME680 measures four quantities and then sleeps. Gas resistance is the resistance of a metal-oxide layer after the internal hot plate has been held at a target temperature (this cog's default is the datasheet quick start: 300 °C for 100 ms). It responds to many volatile compounds. It is not selective for one gas, and it is not a CO2 sensor.

## Good for

- Temperature, relative humidity and barometric pressure at the board
- A raw gas-resistance trend, in ohms, for the same board over time

## Not for

- Deciding whether air is safe to breathe
- Counting people, tracking them, or drawing a box around them
- Indoor-air-quality index, CO2-equivalent or breath-VOC. Those are BSEC outputs. This cog does not compute them.
- 5 V power or 5 V I2C pull-ups

## Datasheet spec (vendor, not our measurement)

Source: Bosch Sensortec BST-BME680-DS001-09, document revision 1.9, February 2024.

| Item | Vendor figure |
|---|---|
| Supply VDD | 1.71 V to 3.6 V |
| I/O VDDIO | 1.2 V to 3.6 V |
| Absolute maximum on VDD or VDDIO | −0.3 V to 4.25 V |
| Operating temperature, full accuracy | −40 °C to 85 °C |
| Humidity | 0 %RH to 100 %RH |
| Pressure, full accuracy | 300 hPa to 1100 hPa |
| I2C address | 0x76 (SDO low) or 0x77 (SDO high) |
| Chip id | 0x61 at register 0xD0 |
| Interface | I2C or SPI. This cog speaks I2C only. |

## What a line looks like

With a sample, stdout is a status object and then a SenML array. `proof_` is `MEASURED` on a real sample and `SYNTHETIC` under `--simulate`.

With no device, stdout is one object: `health` is `no_source`, the four values are `null`, and `error` names `/dev/i2c-1` and the address. There is no second line.
