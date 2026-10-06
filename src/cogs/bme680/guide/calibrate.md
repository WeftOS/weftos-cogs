# Calibrate

> There is no user calibration. The chip's own coefficients are read, not written. This cog does not produce an air-quality index.

The BME680 stores temperature, pressure, humidity and heater coefficients in its registers. The cog reads them and applies the datasheet's floating-point compensation. It never writes those registers, and it never writes the soft-reset register.

## What you can set

| Flag | Default | Meaning |
|---|---|---|
| `--heater-c` | 300 | Hot-plate target in °C, written as `res_heat_0`. Datasheet range used here is 200 to 400. |
| `--heat-ms` | 100 | How long the plate stays hot, written as `gas_wait_0`. The datasheet's 100 ms example is the byte `0x59`. |
| `--source-id` | `bme680/` | The SenML base name. This is the installed board's object id. It must end with `/`. It is not the catalog type `bme680-breakout`. |

The first sample's heater code uses a hardcoded ambient temperature of 25 °C, which the datasheet allows. Later samples use the previous compensated temperature when it was inside −40 °C to 85 °C. That ambient value is not itself a published reading.

Oversampling is fixed at the datasheet quick start: humidity ×1, temperature ×2, pressure ×16. The IIR filter is left off (the reset state), so one sample is not smeared with an older one.

## What you do not get

Bosch's BSEC software computes IAQ, a static IAQ, a CO2 equivalent and a breath-VOC equivalent, and it tracks accuracy and run-in. BSEC is closed source and is not in this cog. Those names are not in the SenML pack. Do not treat gas resistance as an IAQ score. A higher resistance is not "better air" unless you have established that for this board and this heater profile. That has not been measured here.

If `heat_stab_r` is clear, the gas record is omitted and the status is `degraded`. The cog does not fill the gap with zero ohms. Lengthen `--heat-ms` or lower `--heater-c` only after you have a reason to; both are untested on this board.
