# Troubleshoot

> A missing sensor is `no_source` with nulls. A partial sample is `degraded`. Neither one is a row of zeros.

| Health | Meaning |
|---|---|
| `ok` | Temperature, humidity, pressure and gas resistance are all present. |
| `degraded` | The chip answered, but at least one quantity is missing. `error` says which. |
| `no_source` | The bus did not yield a sample. All four values are `null`. Exit code is still 0 for `--once`. |

## Nothing answered at 0x76 on /dev/i2c-1

The error text names the address and the bus, and it names the next physical check. Work down this list.

1. **Wrong bus or port.** The header is `/dev/i2c-1` (`--i2c-bus 1`). A UART cog's `/dev/serial0` is a different problem; this part is not on the serial console.
2. **`/dev/i2c-1` does not exist.** I2C is off, or an OTA or reflash turned it off. Enabling `dtparam=i2c_arm=on` and `i2c-dev` needs the owner's approval. This cog does not edit boot files. Check again after every reflash.
3. **Address.** Default `--i2c-addr 0x76` (SDO strapped to GND). Use `0x77` only when SDO is strapped to VDDIO. Any other address is refused (exit 2) and prints nothing on stdout.
4. **CSB.** CSB must be at VDDIO while the breakout powers up, or the chip locks into SPI and will not answer I2C. This lane could not see that strap on the purple board.
5. **Power-cycle the breakout.** A Seed reboot leaves the 3.3 V pin up, so the sensor does not reset. Unplug the breakout's VDD, or remove power from the Seed entirely.
6. **3.3 V and ground.** Pin 1 and pin 6. Not the 5 V pins.
7. **Another cog** may be using the same bus. Stop it and retry. I2C is shared; this cog does not lock the bus beyond its own sample.
8. **`i2cdetect`** may not be installed. Use this cog's error line and `GET /status` instead.

Chip id other than 0x61 is also `no_source`. The cog will not compensate a different part and call it a BME680.

## Measurement did not finish, or new_data_0 is clear

The cog waited for the `measuring` bit to clear and then required `new_data_0`. If that does not happen, it does not publish the previous contents of the data registers. Power-cycle the breakout. If it keeps happening, the heater time plus the oversampling wait may be too short for a damaged part; that has not been seen on hardware because the part has not been run.

## Gas is null but temperature is present

`gas_valid_r` or `heat_stab_r` was clear, or the heater code could not be computed. The status is `degraded` and the SenML pack has no `gas_resistance` record. That is a failed gas conversion, not zero ohms. The datasheet says a clear `heat_stab_r` means the heat time was too short or the target was too high.

## Quick isolation

1. `--once --simulate` prints a `SYNTHETIC` pack. If that fails, the cog binary is the problem.
2. `--once` with the breakout unplugged prints one `no_source` line. If it prints numbers, something else is answering at that address.
3. `--once` with the breakout powered. Still untested on this hardware.

## Last resorts

- Exit 2 and empty stdout means a refused argument (`--device` outside `/dev`, a bad address, `--api-bind` not on `127.0.0.1`, an unknown flag). Fix the command. The sensor was not opened.
- A `PUT` of the Seed app config replaces every key. Send the whole config, not one field.
- Do not point this cog at a UART device and expect frames. It speaks I2C register reads only.
