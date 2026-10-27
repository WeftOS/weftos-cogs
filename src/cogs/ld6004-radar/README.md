# ld6004-radar

Fail-honest UART reader for a Hi-Link HLK-LD6004 60 GHz 3D presence radar.

The cog resyncs TinyFrame, checks the checksum, and prints the values defined in Hi-Link's HLK-LD6004 presence protocol (cover date September 2025). It does not reuse HLK-LD6002 type-id meanings. `0x0A14` here is the unattended low-power state, not a breathing rate. Heart rate and breathing rate are not in this protocol.

```text
cargo run -- --once --simulate
cargo run -- --once --device /dev/ttyUSB0
cargo test --release --all-targets
cargo test --release --all-targets --features spatial-evidence
```

`--command keep` (the default) writes nothing. `--command query-firmware` sends the documented `0xFFFF` identity query once. Export is `GET` on `127.0.0.1:8055` (`/status`, `/senml`, `/guide`).

`radar_track_point` lines (`spatial.evidence.v1`) are compiled with `--features spatial-evidence` and stay off until `--spatial-out`. No pose means no evidence line. The default uncertainty of 0.3 m is an assumption.

The bench frame (Z forward, up = −Y, camera about 0.30 m above the radar) is in the report's `axes` object. See `MANUAL.md` for wiring, the ESP32-S3 pinout, and the UART viewer. See `docs/adrs/ADR-173-ld6004-radar.md`.

The UART bench tool is `weft-ld6004-scope` in `crates/cog-ld6004-scope`. It is native only.
