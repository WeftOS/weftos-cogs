# ADR-186: can-fd, the node's CAN interface owner

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `can-fd` 0.1.0

## Context

The Ventuno Q's MCU enumerates over USB as `gs_usb` and gives the Linux side three SocketCAN interfaces, `can0` to `can2`, down by default. The CAN sensor-bus spike (card 7794b308) wants frames from a room array of ESP32 nodes and later from a car's OBD-II. Two things limit what the cog may do: no transceiver wiring is confirmed, and transmitting on a bus is WeftOS ADR-123 (governed actuation), which is not approved.

## Decision

1. **One owner, by interface name.** `[[devices]] netdev = "can*"`: a network interface, not a `/dev` node, named in its own namespace. The node refuses a second cog that claims a CAN interface (WeftOS ADR-122 section 4b, which gains the `netdev` selector for this), and hands the owner the names (`can0:can1:can2`, no leading slash) in `WEFTOS_DEVICES`. One instance reads one interface (`--iface`, else the first handed).
2. **Receive only.** A raw SocketCAN socket bound to the interface with `CAN_RAW_FD_FRAMES` and an error filter that asks for every error class. The code has no write path. The report says `transmit: "never"`.
3. **It never configures the interface.** Down, bus-off or missing is `no_source` with the reason; the operator brings an interface up. The controller's state, bit rates and error counters come from `ip -details -json link show`, and `bitrate_matches` compares the kernel's rate with `--bitrate`.
4. **The data.** Each data frame, classic or FD, is a `can.frame.v1` JSON record (at most 512 bytes) in the stream `can.frame/<source>` (source defaults to the interface name). The JSON, not a binary layout, because the registered schema is JSON and a consumer then needs no second parser. Remote frames are counted and not published (the schema has no place for them). `t_ns` is the wall clock when the frame was read, not the controller's hardware stamp.
5. **Counters.** Frames, FD, extended, error frames by class (`controller`, `protocol`, `transceiver`, `no_ack`, `bus_error`, `lost_arbitration`, `tx_timeout`, `bus_off`, `restarted`), bus-off events, the controller's rx and tx error counters, the interface's `rx_dropped` since start, the busiest ids and a bus-load estimate (frame bits plus 20% for stuffing over what the bit rate carries).
6. **Test without a bus.** `--simulate` (three ids, one extended, one FD), `--replay FILE` (a `candump -L` log, including FD lines) and a virtual interface `vcan0`. The virtual interface needs `sudo modprobe vcan; sudo ip link add dev vcan0 type vcan; sudo ip link set up vcan0`, undone with `sudo ip link del vcan0; sudo modprobe -r vcan`; that is a temporary board change the guide documents.

## Consequences

- No hardware timestamp: ordering within a millisecond is the socket's, and `t_ns` jitters with scheduling.
- Load on a bit-rate-switched FD bus is overstated.
- One interface per instance; the claim covers all of `can*`, so a second instance for a second bus cannot be placed until the claim is narrowed per instance.
- A JSON record per frame costs about 150 bytes of ring per frame; at a saturated 500 kbit/s classic bus (4000 frames/s) that is 600 KB/s, which the ring and readers handle but a slow reader will lose frames to (`drop-oldest`).
- `interfaces = ["can"]`.

## Verification

Unit tests cover frame decoding (classic, extended, FD lengths, remote, error frames, bad reads), the `can.frame.v1` JSON, `candump -L` parsing, the `ip` JSON for up, down and virtual interfaces, interface choice, window counters, load, the report and vector. `tests/cli.rs` runs the binary with `--simulate`, a replayed log, a bad log line and a missing interface. On the Ventuno, with `vcan0`: 2417 records through the broker, every one valid against the registered schema, extended and FD frames included, no skips, monotonic times. WeftOS `tests_device_owner.rs` covers the `netdev` ownership rules.
