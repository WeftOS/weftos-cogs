# ADR-187: can-anomaly, ids and rates against a learned baseline

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `can-anomaly` 0.1.0

## Context

Plan 5.1 lists `can-anomaly`: "rate and ID-set baselines, NPU-free". A CAN bus of fixed nodes is very regular: a given id comes at a given rate with a given length. A node that is added, removed, stuck or babbling shows as a new id, a silent id, or a wrong rate, none of which needs a model or the data bytes.

## Decision

1. **A consumer of `can-fd`'s stream.** It reads `can.frame/<source>` as an ordinary reader and owns no device.
2. **Learn, then compare.** For `--learn-s` (30) it counts frames per id (a standard and an extended id with the same number are different ids) and notes the data lengths. It then compares each `--window-s` (5) window with the baseline.
3. **Events**, as `event.v1` readings (a new registered schema: `kind`, `severity`, `subject`, `detail`, optional `value`): `can.baseline` (info, when learning ends), `can.new_id` (warn, once per id), `can.dlc_change` (info, once per length), `can.id_missing` and `can.id_back` (warn and info, for ids that sent 0.5 frames/s or more, silent for two windows or five periods), `can.rate_high` (warn, above 3 times baseline and 5 frames/s over), `can.rate_low` (warn, an id of 2 frames/s or more under a quarter of baseline but still sending), `can.flood` (alert, the whole bus above 3 times its baseline total and 50 frames/s over). Rate events repeat at most every 30 s per id.
4. **The frames' clock.** Time comes from record timestamps, so a replayed log judges exactly as the bus would have; on a live bus that falls silent the wall clock advances the engine so a missing id is noticed.
5. **Counts only.** It never reads the data bytes; a changed value in a known id passes. That is stated in the guide.
6. **A baseline is what it saw.** If the bus was faulty while learning, that is learned as normal; restarting relearns. An id that speaks only on events looks new when it first speaks.
7. **`--simulate`** runs a 30 s synthetic bus with a new id, a silent id and a burst through the engine in virtual time; `--replay FILE` does the same for a `candump -L` log. Both print events and publish nothing.

## Consequences

- Detection delay is one window (5 s) plus the silence limit for a missing id.
- On a bus with irregular ids the baseline produces new-id noise; a longer learning time or a learn-while-busy start helps.
- `event.v1` is general: other cogs can use it.
- `interfaces = ["none"]`; no device, no model.

## Verification

Unit tests cover the engine against quiet and faulty traffic in virtual time (learning ends with one event, normal traffic is quiet, new id once, extended is a different id, length change once per length, missing then back, burst with cooldown, slow but not missing, flood), the options, the event body against the schema's limits, the synthetic bus, replay parsing, stream record parsing, the report and vector. `tests/cli.rs` runs the binary with `--simulate`, a replayed log with an injected id, and no broker. On the Ventuno, with `vcan0` traffic from a generator and `can-fd`: the new id, the silent id, the burst and the flood were reported, every event valid against the registered `event.v1`. WeftOS `contracts/cogs/vectors/event.v1` carries the schema's vectors.
