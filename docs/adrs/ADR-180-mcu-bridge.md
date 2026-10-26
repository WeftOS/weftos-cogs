# ADR-180: mcu-bridge, the node's MCU owner

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `mcu-bridge` 0.1.0

## Context

The Ventuno Q's STM32 MCU is reached through `arduino-router` (msgpack-RPC on `/run/arduino-router.sock`). The brick spike (WeftOS `docs/research/ventuno-q-app-lab-brick-spike.md`) decided that WeftOS owns the MCU: one WeftOS sketch, `mcu-bridge` as the router's client, and a check on start that the sketch is ours. The plan (5.1) lists the sketch's eventual vocabulary (CAN, I2C, ADC, PWM, GPIO); the sketch on the board today has two methods, `weftos_ping` (42) and `weftos_calls` (a counter).

## Decision

1. **One owner.** `[[devices]] socket = "/run/arduino-router.sock"`. The socket is `root:arduino-router 0660`, so the cog's user needs group 127 and nothing else on the MCU side.
2. **The wire.** msgpack-RPC requests `[0, msgid, method, params]`, responses `[1, msgid, error, result]`, one connection per call (1.5 to 5 ms; the spike showed a second client on the socket is fine). `src/msgpack.rs` encodes and decodes the subset the router uses and refuses binary and extension values, non-string map keys, deep nesting and oversize arrays instead of guessing.
3. **The service.** `svc.mcu/<service>` (default `venq-mcu`), schema `mcu.call.v1` (WeftOS `contracts/cogs`): `{method, params}` with a `[a-z][a-z0-9_.]*` method and up to eight scalar params. The reply body is `{result, ms}`; a refusal or router error reaches the caller as `service_error`.
4. **Allowlist.** Only methods on `--allow` (default `weftos_ping,weftos_calls`) are forwarded. The cog re-checks every rule of the schema, because the broker checks only the schema id. A wider sketch vocabulary is enabled by naming its methods, not by default.
5. **Foreign sketch.** Every interval the cog calls `weftos_ping`. 42 is `mcu: ok`; an error from the router or another value is `foreign-sketch` (status `foreign_sketch`, the reason names the fix); no answer in 2 s is `no_answer`; no socket is `no_router` (status `no_source`). The cog never flashes. The sketch source ships in `sketch/weftos_probe/`, and the guide's operator page says what flashing replaces.
6. **Calls run one at a time** in the cog's single loop, which is also the MCU's own serialisation; the loop wakes every 100 ms for calls and runs the probe on its interval.
7. **Reporting.** Probe latency, the sketch's own counter, calls served, failed and refused, bus state, an 8-float vector (id 32) and a `telemetry/<service>` reading.
8. **`--simulate`** uses a fake router; `--replay FILE` answers from JSON lines `{"method", "result"}`. Both are marked.

## Consequences

- The plan's `can.frame.v1`, `senml.v1` and `adc.samples` outputs are not published: the sketch has no such methods yet. They arrive with the sketch, as additions to the allowlist and new outputs.
- A router protocol change (it ships as an apt package) would break the msgpack subset; the brick spike's advice stands: hold the package or reprobe after every update.
- `interfaces = ["mcu"]`; it opens nothing else.

## Verification

Unit tests cover msgpack round trips for every integer width, floats, strings, arrays and maps, known encodings, partial input and hostile input; the router client against a stand-in on a real Unix socket (notifications and other ids skipped, remote errors, a missing socket, a silent router); the fake router; the probe's four outcomes; call validation and the allowlist; options, the report and the vector. `tests/cli.rs` runs the binary against `--simulate`, `--replay`, no socket and a stand-in router socket. The on-board run (the real router, `weftos_ping` through the service) is in the card.
