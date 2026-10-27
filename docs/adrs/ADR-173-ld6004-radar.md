# ADR-173: HLK-LD6004 3D presence radar reader

**Status**: Proposed
**Date**: 2026-10-05
**Cog**: `ld6004-radar` 0.1.0

No card: this cog was requested directly, with no ticket id. The goal is a fail-honest UART decoder for the HLK-LD6004 that does not reuse HLK-LD6002 type-id meanings.

The number is 173, the number this cog's first draft used. A later draft took 169, which `ci-build` now gives to board-telemetry. ADR-172 (ld1125h) and ADR-174 (bme680) are held by other unmerged cog branches.

## Context

The part is a Hi-Link HLK-LD6004, silkscreen V1.0 on the module drawing, a 60 GHz FMCW 3D presence radar. It is a sibling of the HLK-LD6002 in framing only. The LD6002 cog's type ids are a breath and heart protocol. On the LD6004, the same numeric ids are presence, zones, and module settings. This cog does not rename or wrap the LD6002 decoder.

The WeftOS catalog already has module id `hlk-ld6004`. This change adds a cog and a `catalog-fragment.json` that points at that module. It does not edit `catalog.json`. A catalog note that the module "needs 0x08 enable" describes a different Hi-Link behavior. In the LD6004 presence protocol, `0x08` is not an enable this cog sends, and the module auto-streams.

## Sources

Authority, retrieved 2026-10-05:

- Hi-Link, "HLK-LD6004 模组通信协议" (3D presence radar module communication protocol), cover date September 2025. PDF created 2025-10-11. Served from `https://h.hlktech.com/download/HLK-LD6004/1/HLK-LD6004串口通讯协议.pdf`. Product page `https://www.hlktech.com/Goods-347.html`.
- Hi-Link, HLK-LD6004 module datasheet, cover date September 2025. V1.0 first release 2025-09-25. V1.1 size change 2026-08-28. Pin table and header drawing in that datasheet. The module drawing is marked V1.0.

A reseller translation is not used as authority. Checksums below were checked against the protocol's `getCksum` (`!(XOR of covered bytes)`), not against example bytes that fail that algorithm.

## UART

Section 4.2 / 7.2. `0x01 | id:u16be | len:u16be | type:u16be | header checksum | payload[len] little-endian | data checksum if len > 0`. Header checksum covers the first 7 bytes. A zero-length frame has no data checksum. The program default is a total frame of at most 1024 bytes, so a payload longer than 1015 bytes is a desync.

The datasheet states baud 115200 and no parity. It does not state word length or stop bits. Opening 8 data bits and 1 stop bit is an **assumption**. The LD6002's 8N1 is not evidence for this part.

Default `--command keep` writes nothing. `--command query-firmware` writes the documented 8-byte `0xFFFF` query once per open. It changes no setting. Any other command is refused. `--simulate` together with that query is refused. OTA `0x3000`, baud changes, and config writes `0x0201`–`0x0207` are not sent. A received `0x3000` is counted and ignored.

Printed checksums that do **not** match section 7.2, and are not golden vectors: section 6.1 header example `F3` (algorithm yields `F9`); the `0x0A0E` header example; the `0x0A10` header example. Vectors that do match are the `0xFFFF` query `01 00 01 00 00 FF FF FF`, the empty `0x0A08` cloud, the `0x0A0F` fast-trigger example, and the section 5.2 firmware-status example.

The float example `66 66 A2 41` is about 20.3 (IEEE-754 little-endian).

## Type ids implemented

From the September 2025 protocol only:

| Type | Decode |
|---|---|
| `0x0A04` | Targets. `target_num` int32, then N records of x, y, z f32 metres, `dop_idx` int32, `cluster_id` int32. Exact length `4+20*N` or the frame is rejected. Non-finite floats reject the frame. |
| `0x0A08` | Point cloud, same layout. Window only. Not a spatial track. |
| `0x0A0A` | Four uint32 zone words. 1 occupied, 0 empty. Any other value is null for that slot. |
| `0x0A0B`, `0x0A0C`, `0x0A16` | Four rectangles, six f32 metres each (96 bytes), or reject. The table cell that types `x_min` as int32 contradicts the float bullets and the set command. The bullets are followed. `0x0A16` is dwell area, not a distance. |
| `0x0A0D` | Hold delay, uint32 seconds, length 4. |
| `0x0A0E` | Sensitivity, one byte: 0 low, 1 medium, 2 high. |
| `0x0A0F` | Trigger speed, one byte: 0 slow, 1 medium, 2 fast. |
| `0x0A10` | `z_min`, `z_max`, two f32 metres, length 8. |
| `0x0A11` | Install, one byte: 0 top, 1 side. |
| `0x0A12` | Work mode, one byte, names for 0..=4. |
| `0x0A13` | Sleep time. The section heading says `0x0A12`; the TYPE column says `0x0A13`. The example length is 1 byte and the DATA row is a uint32. Decoded only when the payload is 4 bytes, as milliseconds. |
| `0x0A14` | Unattended low-power working state, one byte: 0 low-power, 1 normal. **Not a breathing rate.** A float payload is rejected. |
| `0x0A15` | GPIO mode, one byte, names for 0..=5. **Not a heart rate.** |
| `0x0A17` | Dwell life, uint32 counter. The set command says the counter falls by 20 per second. The integer is emitted. It is not converted to seconds. |
| `0x0A18` | Output interval, uint32, in frames (1 means every frame). The datasheet's 50 ms frame time is a vendor claim, not a protocol constant, and is not applied. |
| `0xFFFF` | Four bytes: project, major, sub, modified. Project names exist for 0..=5. The printed example uses project 6, which has no name. |

A zero-length payload of a known type is an ack. It does not clear the last value. The vendor's "up to 3 people" is a datasheet spec, not a reason to drop a fourth record. Lists are capped at 64 per window so a runaway frame cannot grow the report without a bound. Extra records increment a dropped counter.

### Firmware gate

Section 6 applies to project code 0 (the name table's "presence") and to project code 6. Code 6 is the code in protocol §5.2's own example, and it is the code on the bench module (firmware 3.18.0, payload `06 03 12 00`). The name table still has no label for 6, so `project_name` stays null while `presence_project` is true. Any other project code latches foreign firmware, clears presence values already stored in the window, and is not read as section 6 until a later status is code 0 or 6. The default command does not query, so this latch runs only when a status frame is seen. Without that query, a different project on the same framing could be misread. The guide tells the owner to run `--command query-firmware` once.

Spatial lines already appended cannot be unwritten. A target frame that was emitted before the foreign status in the same read stays in the file. The report for that window does not keep the cleared targets.

## Output

Stdout is one `weavelogic.cog-output.v0` object per window. `timestamp` is Unix seconds. `frame` is `radar_local`. `vitals` is the string `not_defined`. There is no heart-rate field and no breathing-rate field.

When module-status fields arrived, the next line is one SenML array (RFC 8428). `bn` is `--source-id` (default `ld6004-radar`) with exactly one trailing slash. Every record has its own `proof_`: `MEASURED` for UART frames, `SYNTHETIC` for `--simulate`. `u` is `s` for hold delay and `m` for the Z limits. Milliseconds, the dwell-life counter, the output-interval count, and the enum codes have no IANA SenML unit, so those records omit `u`. Positions, zones, and point clouds are not SenML. An empty status produces no SenML line. `--once` with no device is exactly one JSON object and exit 0.

`source.verified` is false when the window has no valid frames or the line is simulated. `--simulate` feeds the real parser: the empty-cloud and fast-trigger examples copied from the manual, plus one synthetic `0x0A04` and one synthetic `0x0A14` built with the checksum function. The project-6 firmware example is not in that burst, because it would latch the foreign gate.

Export is HTTP/1.0, GET only, `127.0.0.1:8055`. A non-loopback bind is refused. A bind failure is logged and the cog still prints stdout. `GET /guide` serves this cog's ADR-104 guide. `GET /spatial` exists only in the `spatial-evidence` build.

## ADR-384

RuView ADR-384 (external sensor evidence ingest) is applied only where this part's manual matches.

Emitted spatial type: **`radar_track_point` only**, from `0x0A04`, and only when the crate is built with `spatial-evidence` and `--spatial-out` is set. The feature is off in the default build. With the feature, `--spatial-out` still defaults to off.

Not emitted: `radar_track` with a velocity, point-cloud types, zone occupancy as evidence, and any vital-sign type. ADR-384 §3: heart rate and breathing rate are not spatial evidence. This manual does not define those rates, so there is no SenML vital pack and no invented unit `bpm`.

No pose means no evidence line. The report's `spatial` field is `no_pose` (or `no_region`). One cluster id is a radar track, not a person. Negative cluster ids are kept in the window and skipped for evidence. `dop_idx` is not copied onto the track as a velocity.

The room transform uses the frame measured on the bench, not a side-mount guess that treats Y as boresight. Z is forward, out of the antenna. X is lateral (`right = x_sign * x`). +Y points toward the bottom edge of the board when the silkscreen reads normally, so height above the radar is `-Y`. At pitch 0 the boresight is horizontal: yaw turns Z into east/north, and room up is `-Y`. The five-number pose sets pitch to 0. `x_sign` is configured. The install byte is reported and does not swap axes. A paired camera sat about 0.30 m above the radar (`camera_above_radar_m`). That height is not a bias subtracted from X. A sideways return stays in X.

Default `uncertainty_m` is 0.3. That is an assumption, not the vendor's accuracy figure. Beam half-angles of ±60° are the vendor datasheet §4.3, carried on the `sensor` block, and are not a measurement made here. Room coordinates are rounded to 0.001 m and refused outside ±1000 m. Lines longer than 16 KiB are dropped. `source_id` is the installed object id with any trailing slash removed. `proof` is `MEASURED` or `SYNTHETIC`. `CODE` is not used. `SYNTHETIC` and `MEASURED` are not a claim of a bench test.

Local serde only. No weftos or ruview crate dependency.

## Wiring assumptions

Datasheet header, pin 1 OUT (GPIO, not UART), pin 2 TX0 to the host RX, pin 3 RX0 (the description column repeats `uart0_txd`; wire by pin name), pin 4 GND, pin 5 3V3. Supply 3.1–3.5 V, at least 1 A, ripple at most 50 mV: vendor limits, not measured. 5 V is forbidden. `/dev/serial0` on a Seed stays the serial console until the owner changes boot configuration. This cog does not.

## Unknown

- Unit and scale of `dop_idx`. Omitted from SenML and from the spatial line. The integer is kept on the stdout target.
- Which physical edge is to the right of +Z once the board is turned in the room. Configured as `x_sign`. On the bench, +X pointed toward the silkscreen text.
- A wall side-mount at 1.5–2 m. Not measured. The install byte does not select a second formula.
- `0x0A13` length: the example and the DATA row disagree. Only a 4-byte payload is decoded.
- RX0 description text says `uart0_txd`. Wiring follows the pin name.
- Firmware example project code 6 is outside the 0..=5 name table. Stored with a null name and accepted as this presence protocol.
- Baud-index 2 is printed as `128300`. No baud command is sent. Listen baud stays in 9600..=921600, default 115200.
- Whether a point-cloud count may exceed 3. A legal extra record is kept up to the 64 cap.
- SenML has no unit for milliseconds, dwell life, output interval, or the enum codes. `u` is omitted.
- 8 data bits and 1 stop bit. The datasheet does not state them. The bench unit answered at 115200 8N1.
- 0.3 m uncertainty. Assumption.
- Whether the Pi 3.3 V pin can feed the vendor's current. The guide says to use a supply that can, and to share ground.
- This cog binary on a Seed or a Pi 5. Not run. No captured frame file is in the tree.

## Verification

MEASURED 2026-10-10, Mac USB-UART, 115200 8N1, module silkscreen V1.0. Firmware project 6, version 3.18.0. Install byte 0 (top). The UART viewer, not this cog binary, held the port. One still return sat about a metre forward, with Y near −0.32 m. That height matches a camera about 0.30 m above the radar. The lateral reading was the return, not a mount offset, and is not subtracted. Seed armhf and Pi 5 aarch64 are not tested. `--simulate` is labelled synthetic and is not that measurement.

## Consequences

Operators get a decoder that says when it has no source, when a payload does not match the manual, and when the firmware project is not presence. They do not get a vital sign, a person id, or a room track unless they opt into the spatial feature and supply a pose. The guide ships inside the binary at `GET /guide`.
