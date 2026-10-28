# Start here

> Read temperature, humidity, pressure and gas resistance from a Bosch BME680 on a Cognitum Seed. The numbers describe the board's air, not a person.

The `bme680` cog takes one forced-mode sample over I2C and prints a status line plus a SenML pack (RFC 8428). Temperature is in `Cel`, humidity in `%RH`, pressure in `Pa`, and gas resistance in `Ohm`. It does not emit `spatial.evidence.v1`, a track, or a bounding box. ADR-107 has no record type for these quantities.

Hardware is not yet tested. `--simulate` runs the real compensation math on a fixed synthetic calibration image and labels the pack `SYNTHETIC`. That proves the cog, not the sensor.

## The 5-minute path

1. **Identify the board.** A purple breakout whose chip is marked BME680. → see **Parts**
2. **Wire 3.3 V, ground, SDA and SCL.** Do not use 5 V. Strap CSB and SDO on the breakout. → see **Wiring**
3. **Confirm I2C is enabled** on the Seed (`/dev/i2c-1`). Enabling it is an owner approval. This cog does not edit boot files. → see **Set up**
4. **Try the software with no sensor:** `cog-bme680 --once --simulate`. → see **Set up**
5. **One real sample:** `cog-bme680 --once`. A missing board prints one `no_source` line and exits 0. It does not print zeros. → see **Troubleshoot**

## How the data flows

```diagram
flow
```

The part wakes, measures temperature, pressure and humidity, heats the gas sensor, measures its resistance, and returns to sleep. The cog compensates the raw ADC values with the calibration bytes stored on the chip.

## What you get

| Output | Where | What it is |
|---|---|---|
| Status object | stdout, one JSON object | `health` of `ok`, `degraded` or `no_source`. Missing quantities are `null`. |
| SenML pack | the next stdout line, or `--readings-out` | One JSON array. Object readings, not people. |
| Export | `http://127.0.0.1:8057` on the Seed | `GET /status`, `GET /readings`, `GET /guide` |

→ see **API** for every field.

## Honest limits

- **Not yet tested** on a Seed or on this breakout. No measured error against a reference thermometer, hygrometer or barometer.
- **Not an air-quality index.** Bosch's IAQ, CO2-equivalent and breath-VOC figures come from the closed-source BSEC library. This cog does not run BSEC and does not invent those numbers.
- **Not presence.** A temperature is the state of this object. It does not mean a person is here.
- **The purple board's power path was not visible** to this lane (regulator versus the chip's raw 1.71–3.6 V pin). Treat the header as 3.3 V only.
