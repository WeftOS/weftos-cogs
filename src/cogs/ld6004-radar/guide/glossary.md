# Glossary

> Short names used by this cog, with the meaning they have for the LD6004.

**TinyFrame.** The UART framing in the HLK-LD6004 protocol: start byte `0x01`, big-endian header, inverted-XOR checksum, little-endian payload.

**Presence project.** Firmware project codes 0 and 6. Section 6 applies to both. Code 6 has no name in the protocol table. Other project codes are foreign firmware for this cog.

**Cluster id.** The int32 on an `0x0A04` record. When it is >= 0 it becomes the `track` on a `radar_track_point` line. It is not a person identifier.

**dop_idx.** The int32 Doppler index in the same record. The protocol does not state a unit or a scale. The cog stores the integer and does not turn it into a velocity.

**Radar-local.** Metres in the module's axes, measured on the bench with the silkscreen readable: X lateral toward the text, Y toward the bottom edge, Z forward out of the antenna. Height above the radar is `-Y`. The stdout field `frame` is `radar_local`. `above_radar_m` is that height.

**Pose.** Where the radar is mounted in room coordinates, plus yaw, pitch, and `x_sign`. Required before any evidence line. Pitch 0 keeps Z horizontal. `x_sign` is +1 when +X is to the right looking along +Z.

**radar_track_point.** The only `spatial.evidence.v1` type this cog emits. It comes from `0x0A04` only.

**SenML.** RFC 8428 records for module status. `bn` ends with `/`. `proof_` is on every record. Not used for positions or for vital signs.

**proof.** `MEASURED` for frames taken from the UART. `SYNTHETIC` for `--simulate`. Neither one is a claim that hardware has been bench-tested. `CODE` is not used.

**0x0A14.** On this module, the unattended low-power working state, one byte. 0 is low-power, 1 is normal.

**Keep.** The default `--command`. The cog writes zero bytes to the module.

**not_defined.** The report's `vitals` value. Heart rate and breathing rate are outside this protocol.

**Vendor figure.** A number copied from the Hi-Link datasheet or protocol. It is not a measurement made with this cog. Hardware is not yet tested.
