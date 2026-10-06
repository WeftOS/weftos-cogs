# Glossary

> Names used in this cog, with the meaning this guide actually uses.

| Term | Meaning |
|---|---|
| SenML | RFC 8428 sensor markup. This cog writes one JSON array (a pack) per sample. |
| `bn` | Base name. The installed board's object id, default `bme680/`. It must end with `/` so it does not glue onto `temperature`. |
| `proof_` | On every record. `MEASURED` for a real I2C sample, `SYNTHETIC` for `--simulate`. It does not carry from the first record to the next. `CODE` is not used. |
| `Cel` | SenML unit for degrees Celsius. Not `C` and not `degC`. |
| `%RH` | SenML unit for relative humidity. |
| `Pa` | Pascal. 100 Pa is 1 hPa. The cog does not convert to hPa. |
| `Ohm` | Ohm, the gas-sensor resistance after compensation. |
| Forced mode | One temperature, pressure, humidity and gas cycle, then the chip returns to sleep. |
| `no_source` | The sample did not happen. Values are null. |
| `degraded` | The chip answered, but at least one quantity is missing. |
| CSB | Chip-select. Tie it to VDDIO or the part selects SPI and ignores I2C until power is removed. |
| SDO | Address strap. Low is 0x76, high is 0x77. Not a Pi GPIO in this wiring. |
| Chip id | Register 0xD0. The datasheet's reset value is 0x61. |
| BSEC | Bosch's closed-source library for IAQ and related indexes. Not linked into this cog. |
| Object reading | The state of a thing (this board's air). Not evidence that a person is present. |
| `spatial.evidence.v1` | The perception-evidence line this cog does not emit. |
