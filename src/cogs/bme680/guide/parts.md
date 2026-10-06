# Parts

> Purple BME680 breakout, four jumper wires, and a Cognitum Seed. The chip mark is the identification this cog relies on.

| Part | Role |
|---|---|
| Breakout with a chip marked BME680 | The sensor. Purple PCB in the owner's notes. This lane did not inspect that photo. |
| Four jumper wires | 3.3 V, ground, SDA, SCL |
| Cognitum Seed | Pi Zero 2 W header, I2C bus 1 (`/dev/i2c-1`) |

## What this lane could not see

The task describes a purple breakout and a chip marked BME680. The album photo was not available here, so none of the following were read off the board:

- Whether a regulator sits between the header VCC and the chip, or the header is the chip's raw VDD (1.71 V to 3.6 V)
- The silkscreen names (VCC versus VDD, SDA versus SDI, and so on)
- Whether SDO is strapped to ground (address 0x76) or to VDDIO (address 0x77)
- Whether CSB is tied to VDDIO. If it is not high at power-up, the chip locks into SPI until the breakout itself loses power.

Trust the labels printed on your board over the datasheet nicknames in **Wiring**, and over wire colours.

## Look-alikes

The BME280 is temperature, pressure and humidity only: no gas, and a different chip id. The BME688 is a later gas sensor. Bosch's BME68x API uses chip id 0x61 for both the BME680 and the BME688. This datasheet does not define a register that separates them. The cog checks for 0x61 and then speaks the BME680 forced-mode sequence. It does not implement BME688 scan mode. If the chip is not marked BME680, do not assume these numbers.

A BMP280 or BMP388 is pressure (and sometimes temperature) only and will not answer as chip id 0x61. The cog refuses any other id and does not invent a sample.
