# Set up

> Install the cog, confirm the bus, then take one sample. Simulation does not need the sensor.

## Check the bus

On the Seed, `/dev/i2c-1` should exist. If it does not, I2C is off or a reflash removed it. Turning it on changes boot configuration and needs the owner's approval. This cog will not do that.

`i2cdetect` is not assumed to be installed. The cog's own `no_source` line and `GET /status` are the probe.

## Sideload

Cross-build and sideload are done by the seed operator, with approval, because they write onto the Seed:

```text
scripts/cross-build.sh bme680
scripts/seed-sideload.sh bme680 genesis@<seed>
```

`<seed>` is the device's name. Do not put a LAN or tailnet address in this guide.

## Simulate, then one real sample

On a machine without the sensor, or from the Seed console:

```text
cog-bme680 --once --simulate
```

Expect two lines. The status object has `"simulated": true` and `"health": "ok"`. The second line is a JSON array whose every record has `"proof_": "SYNTHETIC"`. The numbers are the compensation of a fixed fake calibration image. They are not the air in the room.

With the breakout wired:

```text
cog-bme680 --once
```

A real sample has `"proof_": "MEASURED"` and `"simulated": false`. `"verified": false` stays false until someone has compared the cog with a reference instrument on this hardware. That has not happened yet.

If the board is missing, the command still exits 0 and prints one line: `"health": "no_source"`, four nulls, and an `error` that names the bus and the address. No SenML line.

## Run continuously

```text
cog-bme680 --interval 10
```

Each interval is one forced-mode measurement. The export at `http://127.0.0.1:8057` on the Seed is up before the sensor is opened, so a missing board is visible at `/status` instead of a refused connection.

The console may run only the commands listed in `cog.toml`: `--help`, `--once`, and `--once --simulate`.
