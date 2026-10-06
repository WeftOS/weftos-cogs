# Troubleshoot

> `no_source` means the tty did not open. `no_frames` means it opened and no accepted line arrived. Silence is not an empty room.

| What you see | Next check |
|---|---|
| `no_source` | The path is under `/dev` and the adapter is plugged in. Error text names UTX to Pi pin 10, ground to pin 6, and 5 V from pin 2 |
| `no_frames`, `undecoded_lines` is 0 | The module stops sending when it detects nothing (manual V1.2). Confirm power, baud 115200, and that UTX (not URX) is on the Pi's RX |
| `no_frames`, `undecoded_lines` > 0 | Bytes arrived and were not `mov, dis=` or `occ, dis=`. Command replies and `str` suffixes are not readings |
| `rejected_lines` > 0 | The prefix matched and the distance was not a plain decimal in `(0, 100]`. `mov, dis=***` counts here |
| Another program holds the tty | Only one process can own `/dev/ttyUSB0` or the header UART |
| Export missing | Port 8054 was busy at start. Stdout still has the report. The bind is loopback only |
| Spatial file empty | No `--spatial-out`, no pose, no region, or the module sent no accepted line. The stdout `spatial` field says `no_pose` or `no_region` when that is the reason |

Do not send `rmax`, `mth1`, `save`, or `test_mode` from this cog. The manual's threshold commands disagree with each other (`mth1_mov` defaults versus the demo and `test_mode` prose). This decoder does not pick a winner.

`--simulate` is labelled simulated. It is not a hardware pass.
