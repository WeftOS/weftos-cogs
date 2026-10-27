# Setup

> Install the cog, then run `--once`. The default command writes nothing to the radar.

The binary is `cog-ld6004-radar`. On a Seed the installed name is `cog-ld6004-radar-arm`. Version 0.1.0 matches `cog.toml`.

```text
cog-ld6004-radar --help
cog-ld6004-radar --once --simulate
cog-ld6004-radar --once --device /dev/ttyUSB0
cog-ld6004-radar --once --command keep
```

`--simulate` does not open the serial port. It feeds two frames copied from the protocol and two frames built with the same checksum function, through the real parser. The report's `source.simulated` is true, `source.verified` is false, and `reasons` includes `simulated`. Spatial lines from that run are `SYNTHETIC`. Simulated output is not the sensor and is not ground truth.

`--once` with a device path that cannot be opened prints one JSON object, `health` `no_source`, null quality, `verified` false, and exits 0. It does not invent a target.

`--command query-firmware` writes the 8-byte identity query once after a successful open. Combine it with a real device, not with `--simulate` (that combination is refused, exit 2). Read the report's `firmware` object. `presence_project` true means project code 0 or 6.

The UART viewer is separate from this cog. From the repository root:

```text
cargo run -p cog-ld6004-scope --bin weft-ld6004-scope -- --port /dev/ttyUSB0
```

On the bench Mac the adapter was `/dev/cu.usbserial-214110`. The viewer can send module commands when you click them. This cog does not. See `MANUAL.md`.

`--source-id` is the id of this installed radar. The default is `ld6004-radar`. SenML `bn` is that id with one trailing slash. It is not a catalog type and not a person.

The export listens on `127.0.0.1:8055` unless `--api-bind` names another loopback port. Any other address is refused. If the port is already taken, the cog says the export is disabled and still prints stdout.

A continuous run is the same command without `--once`. The default window is 1 second (`--interval`).

Building the room-track adapter:

```text
cargo build --release --features spatial-evidence
```

The default build has no spatial flags. Passing `--spatial-out` to that binary exits 2 with empty stdout. With the feature, `--spatial-out` still defaults to off.

This repository does not change the Seed's boot partition, and this page does not sideload firmware. Installing the binary on a Seed is a separate owner step.
