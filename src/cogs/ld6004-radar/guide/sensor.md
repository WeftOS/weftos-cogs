# The sensor

> HLK-LD6004 speaks its own TinyFrame protocol. Type ids from the LD6002 breath radar do not mean the same thing here.

Hi-Link describes the part in "HLK-LD6004 模组通信协议" (3D presence radar module communication protocol), cover date September 2025, and in the module datasheet V1.1 (size change dated 2026-08-28). The product page is <https://www.hlktech.com/Goods-347.html>. This cog follows those two documents.

The module is a 60 GHz FMCW presence radar. It reports target positions, a point cloud, four zone flags, and its own settings. It streams without a host enable command. A catalog note that this module needs a `0x08` enable belongs to a different Hi-Link part and is not applied here.

## Type ids this cog decodes

| Type | Meaning on the LD6004 |
|---|---|
| `0x0A04` | Target positions: x, y, z in metres, a Doppler index, a cluster id |
| `0x0A08` | Point cloud, same layout. Kept in the report. Not emitted as a track. |
| `0x0A0A` | Four zone words, 1 occupied, 0 empty |
| `0x0A0B` / `0x0A0C` / `0x0A16` | Interference, detection, and dwell rectangles |
| `0x0A0D` | Hold delay, seconds |
| `0x0A0E` | Sensitivity 0 low, 1 medium, 2 high |
| `0x0A0F` | Trigger speed 0 slow, 1 medium, 2 fast |
| `0x0A10` | Z limits, metres |
| `0x0A11` | Install method 0 top, 1 side |
| `0x0A12` | Work mode 0 through 4 |
| `0x0A13` | Unattended sleep time, decoded only as a 4-byte millisecond count |
| `0x0A14` | Unattended low-power working state: 0 low-power, 1 normal |
| `0x0A15` | GPIO mode 0 through 5 |
| `0x0A17` | Dwell life counter |
| `0x0A18` | Output interval, in frames |
| `0xFFFF` | Firmware project and version |

`0x0A14` on this part is the low-power state. A payload that is a float is rejected. `0x0A15` is the GPIO mode. `0x0A16` is the dwell rectangle, and a 4-byte payload is rejected.

Heart rate and breathing rate do not appear in this protocol. The report field `vitals` is the string `not_defined`. They are not written as SenML and they are not attached to a spatial track.

## Firmware project

Section 6 applies to project code 0 and to project code 6. Code 6 is the protocol's own firmware example and the code the bench module returned (version 3.18.0). The name table has no label for 6, so the report leaves `project_name` null and sets `presence_project` true. `--command query-firmware` sends the documented 8-byte `0xFFFF` query once after the port opens. It changes no setting. Any other project code stops presence decoding and clears presence values already stored in that window. Lines already written to a spatial file stay written.

The default is `--command keep`, which does not send the query. Until a status frame is seen, the cog decodes section 6. Run the query once on a real module before trusting the decode.

## Frame rules

A frame is `0x01`, a big-endian id, length, and type, a header checksum, then the little-endian payload and a data checksum when the payload is not empty. The checksum is the inverted XOR of the covered bytes. On a mismatch the cog drops one byte and scans again. A length that would make the whole frame longer than 1024 bytes is rejected. Floats are IEEE-754 little-endian. The worked example `66 66 A2 41` is about 20.3.

Some printed examples in section 6 do not match that checksum. The cog follows the stated algorithm. Those examples are not used as test vectors.

`0x3000` is an OTA command. The cog does not send it and does not act on it. A received frame is counted only.
