# bme680

Bosch Sensortec BME680 over I2C (`/dev/i2c-1`, address `0x76` or `0x77`). One forced-mode sample becomes a status JSON object and a SenML pack: temperature (`Cel`), humidity (`%RH`), pressure (`Pa`), gas resistance (`Ohm`).

Not a presence sensor. No `spatial.evidence.v1` output. See `docs/adrs/ADR-174-bme680.md`.

```text
cog-bme680 --once --simulate
cog-bme680 --once
```

Export: `127.0.0.1:8057` (`/status`, `/readings`, `/guide`).

Protocol: BST-BME680-DS001-09, revision 1.9, February 2024. Heater-code and calibration layout also cite Bosch BME68x Sensor API v4.4.8 (`80ea120`).

## Not yet tested

The physical breakout has not been read. `--simulate` checks the compensation math on synthetic calibration and ADC bytes. It does not check the sensor. Whether the purple board regulates 3.3 V or wires the header straight to the chip was not visible.
