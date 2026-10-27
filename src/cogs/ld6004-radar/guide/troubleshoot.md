# Troubleshoot

> A missing device is a `no_source` line, exit 0. A bad argument is exit 2 and no JSON.

| What you see | What it means |
|---|---|
| `health` `no_source`, reason `device_error` | The path could not be opened. Check the path, the adapter, and that another process is not holding it. |
| `no_bytes` or `no_valid_frames` | The port opened and no checksum-valid frame arrived in the window. Power-cycle the radar, not only the Seed. Confirm TX0 is on the host RX pin, at 115200. |
| `checksum_errors` / `degraded` | Bytes arrived and the checksum failed. Usually a swapped TX/RX, a wrong baud, or a 5 V level. |
| `foreign_firmware` | A `0xFFFF` reply named a project other than 0 or 6. Presence fields stay empty until a 0 or 6 status arrives. |
| `payload_rejected` | A known type arrived with a length the protocol does not use. The value stays null. |
| `spatial` `no_pose` | `--spatial-out` is set and `--radar-pose` is not. No evidence file is created. |
| Exit 2, empty stdout | Unknown flag, device outside `/dev/`, non-loopback bind, or `--simulate` together with `--command query-firmware`. |
| Export disabled on stderr | `127.0.0.1:8055` is in use. Stdout still works. |

The Seed console owns `/dev/serial0` and `/dev/ttyS0` until that ownership is changed by the owner. A cog pointed at the console device will fail to open it or will see console text, which fails the checksum. Use `/dev/ttyUSB0` or another USB-serial node.

There is no I2C scan for this sensor. `i2cdetect` will not show it.

`0x0A14` in the frame counts is the low-power state, a one-byte value. If you expected a breathing rate, that expectation comes from a different module.

Vendor frame examples that fail the XOR-NOT checksum are not accepted. A silent module can still be a good module that has not been power-cycled after wiring.

The bench check on 2026-10-10 used the UART viewer, not this binary. A green `--simulate` run does not replace that, and it does not replace a Seed run.
