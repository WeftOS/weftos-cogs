# ADR-172: ld1125h-radar, Hi-Link HLK-LD1125H-24G ASCII presence lines over UART

**Status**: Proposed
**Date**: 2026-10-05
**Cog**: `ld1125h-radar` 0.1.0

No card id was assigned for this cog. ADR-172 is the next free number in this worktree after the existing ADR files (166 is already used twice). The filename matches the gate glob `docs/adrs/ADR-*<cog-id>*.md`.

This ADR records decisions in `src/cogs/ld1125h-radar/`. It does not add a device driver to RuView and it does not edit the WeftOS catalog. Module id `hlk-ld1125h-24g` already exists; `catalog-fragment.json` is a project pointer only.

## Context

The part is the Hi-Link HLK-LD1125H-24G, a 24 GHz FMCW human-presence radar. The owner's board is a blue PCB about 31 × 16 mm, four gold patches, silk `90-00074`, pins along the long edge marked `5V G UR UT`. Board size and silk are from that board. Manual V1.2 does not include a dimensioned drawing.

The protocol source is Hi-Link, "Human Presence Detection Radar HLK-LD1125H-24G Application Manual", V1.2, 2021-9-16. The PDF is not committed. Community pages are not a source. Sibling LD11xx modules are not this document. Framing alone does not identify the part.

The manual's method for a still person is breathing micro-motion ("breathing induction"). That phrase names the method. It is not a rate and not a spatial vital-sign record (RuView ADR-384 §3). The module does not report a heart rate. This cog has no heart-rate or breathing-rate field, not even null.

## What the manual specifies

| Fact | Value | Where used |
|---|---|---|
| Supply | 3.3–5 V, about 80 mA typical; vendor debug diagram uses 5 V | Guide: VCC from Pi pin 2. Pin 1 is avoided |
| UART | 3.3 V, factory 115200 8N1, ASCII | `--baud` default, `serial.rs` |
| Larger motion | line `mov, dis=***` | `lines.rs`, status `mov` |
| Still or small motion | line `occ, dis=***` | `lines.rs`, status `occ` |
| `dis` | target distance, metres, described as approximate | `distance_m` and, when enabled, `radar_range.range_m` |
| Silence | the module stops sending when it detects nothing | `no_frames`, not a cleared room |
| Beam | antenna half-power angle ±22° horizontal and vertical | `fov_deg = [44, 44]`, the full width, same reading as the RD-03E "±" angles. Vendor claim |
| Pins | 1 VCC, 2 GND, 3 URX, 4 UTX, 2 mm pitch | Guide wiring. URX is left open |
| Install | horizontal about 1 m facing the radar, or hanging about 3 m | Guide mounting. Not measured here |
| Commands | `rmax`, `mth*`, `save`, `get_all`, `test_mode`, replies such as `rmax is 8.00` | Not sent. Replies are undecoded |

The asterisks are the manual's placeholder. There is no numeric worked example. Tests use synthetic decimals (`1.25`, `0.75`) in the manual's spelling.

## Unknown

Marked unknown rather than filled in:

- The byte that ends a **report** line. Commands are specified to need CR+LF. The parser assumes LF and strips one preceding CR.
- Numeric precision of `dis`. Only `***` is printed.
- Whether `str` has a wire form. `test_mode` prose mentions `dis` and `str`. A suffix on a `dis` line is rejected. No signal-strength field is stored.
- Threshold commands contradict each other in the same manual (directive defaults, the demo, and `test_mode` prose). The cog does not choose a default and does not send them.
- Maximum reportable distance. `(0, 100]` metres is this cog's refusal bound, not a datasheet maximum. The manual's longest discussed figure is the third threshold segment, "above 8 m", and the demo default `rmax` is 6.
- Report rate. The detection cycle is adaptive. No hertz is taken from the manual. `--spatial-max-hz` defaults to 2 as a host limit.
- Startup delay. Not stated.
- Multiple targets. The report text is singular. One accepted line is one distance. `targets: 1` means one range line, not a person id.
- Whether baud is configurable. Only 115200 is documented. `--baud` is an operator override.
- Whether sibling modules emit the same lines.
- A 1σ for distance. Default `uncertainty_m` is 1.0 and is labelled assumed, overridable with `--spatial-uncertainty-m`.
- FCC or CE. Not in this manual.
- Hardware behaviour. **Not yet tested.** No MEASURED block. Not yet tested on Seed armhf or Pi 5 aarch64.

## Decision

`ld1125h-radar` is the decoder. It opens the UART, keeps one accumulator, and parses. It does not add an RVF WASM producer and it does not `io_read` from WASM.

**Read only.** The tty is opened read-write because setting the baud needs a writable fd (`serial.rs`). There is no write method. Configuration commands are not sent. Pi pin 8 (TX) is not wired to URX, because a getty can emit those commands. `/dev/serial0` stays the console until the owner frees it. The default device is `/dev/ttyUSB0`.

**Parser** (`lines.rs`). LF ends a line; a preceding CR is stripped. Blank lines are ignored. A line longer than 128 bytes is rejected. Noise with no newline is dropped one byte at a time once the buffer passes 128, so the buffer cannot grow without bound. The grammar is exactly `mov, dis=<decimal>` or `occ, dis=<decimal>`. Anything else with that prefix is rejected (the distance was not usable). Other text is undecoded, including command feedback. There is no checksum; `checksum_errors` stays 0. Non-finite values cannot appear from the plain-decimal grammar and are refused again at the numeric check. A value the module did not report is null, not 0, and a quiet window does not reuse the previous distance.

**Stdout** is one JSON object per window. `status` is `no_source`, `no_frames`, `mov`, or `occ`. `--once` with no device prints one `no_source` line and exits 0. Unknown arguments and a device path outside `/dev/` exit 2 with empty stdout. Export binds `127.0.0.1:8054` before the sensor open (`bind_loopback_only`). Store ingest uses vector id 28 and only after an accepted line; failure is stderr. The vector is a kind code and a normalised distance, not a person score.

**Spatial evidence** (RuView ADR-384, WeftOS ADR-107 §7.1). Cargo feature `spatial-evidence` is off in the default build, so the default binary rejects `--spatial-out` as an unknown argument. With the feature, emission stays off unless `--spatial-out` is set (`export` or a file path; not `/dev`, `/proc`, `/sys`, and no `..`).

An accepted distance may be one `spatial.evidence.v1` line of type `radar_range`:

- `position` is the antenna from `--radar-pose` (`x,y,z,yaw[,pitch]`, room ENU). It is not a point slid out to `range_m`. There is no `x_sign`, because a sign would invent a cartesian target.
- `range_m` is `dis`.
- `fov_deg` is `[44, 44]`.
- `frame` is `room_enu`. `targets` is 1.
- `source_id` defaults to `ld1125h-1` and is the installed object, not the catalog type and not a person.
- Proof is `MEASURED` for a real frame and `SYNTHETIC` under `--simulate`. Neither is ground truth.
- No pose: emit nothing and set the report's `spatial` to `no_pose`. No region: `no_region`. A silent window writes no line.

`mov` and `occ` stay on the cog's JSON report. They are not `human_confirm`, not `radar_track_point`, and not a person.

**No SenML.** ADR-384 allows a SenML pack only for a non-spatial quantity the manual defines, with `bn` ending in `/` and a `proof_` on every record. This manual's distance is spatial. It defines no separate non-spatial quantity, so this cog emits no SenML line and invents no unit. The two outputs are never mixed: the stdout object is the cog report; `radar_range` goes to the file or `GET /spatial`.

**Guide.** `guide/guide.toml` schema 1 and the markdown pages are embedded by `src/guide.rs` and served at `GET /guide` as `{"toml","pages"}` (WeftOS ADR-104).

## Consequences

Operators who want a track, a pose skeleton, or a vital sign need a different sensor. This part's wire output is a class word and one approximate range. A downstream consumer that treats `occ` as "a person is here" is adding a claim this cog refused to make.

Hardware proof is still outstanding. `--simulate` and host tests do not substitute for a module on a Seed.
