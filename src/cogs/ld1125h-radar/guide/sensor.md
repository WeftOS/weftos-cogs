# The sensor

> Hi-Link HLK-LD1125H-24G, application manual V1.2 (2021-9-16). Decode only what that document defines. Hardware on this cog is not yet tested.

The part is a 24 GHz FMCW human-presence radar. The manual's detection method for a still person is breathing micro-motion ("breathing induction"). That phrase is the method. It is not a breaths-per-minute field and not a spatial vital-sign record. The module does not report a heart rate.

Vendor claims in that manual, not measurements made here:

| Fact | Manual |
|---|---|
| Band | 23.5–24.5 GHz FMCW |
| Detection (vendor) | about 4 m while meditating, about 8 m while exercising |
| Hanging height | 3 m, static coverage radius greater than 2 m |
| Supply | 3.3–5 V, about 80 mA typical |
| UART | 3.3 V, factory 115200 8N1, ASCII |
| Beam | antenna half-power angle ±22° horizontal and vertical |
| Distance | approximate. The manual prints `dis=***`, not a numeric example |

## The lines this cog accepts

Exactly `mov, dis=<decimal>` or `occ, dis=<decimal>`: lowercase, one space after the comma, no space around `=`. The decimal is digits with an optional fraction. `mov` is a larger motion. `occ` is a still body or a small motion. `dis` is metres.

The manual's asterisks are a placeholder. `mov, dis=***` is not a reading. Zero, a sign, an exponent, and values outside `(0, 100]` are refused. 100 m is this cog's refusal bound, not a datasheet maximum.

When the module detects nothing it stops sending. Silence is `no_frames`.

## What is not decoded

| Item | Why |
|---|---|
| `str` | Named in the `test_mode` prose. No byte layout in the manual. A suffix on a `dis` line is rejected |
| `rmax`, `mth*`, `save`, `get_all`, `test_mode` | Configuration the cog does not send. Command replies are undecoded text |
| Line terminator | Not specified for report lines. This parser assumes LF and strips a preceding CR |
| Report rate | The manual says the detection cycle is adaptive. No hertz figure is taken from it |
| Multiple targets | The manual's report text is singular. One accepted line is one distance |

Sibling modules (other LD11xx parts) are not this manual. Framing alone does not identify the part.
