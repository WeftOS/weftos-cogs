# LD1040ADC protocol note — stopped, no cog

**Date:** 2026-10-05
**Branch:** `feat/ld1040adc-cog`
**Status:** stopped before a decoder. No `src/cogs/ld1040adc`, no catalog fragment, no ADR number.

The photographed part is named **HLK-LD1040ADC V1.1**. That string does not appear on Hi-Link's own 10G catalog or in either datasheet below. A renamed copy of `ld1040c-motion` is not this part's protocol. Hardware was not tested. Nothing was pushed.

## What was checked

Hi-Link's 10G radar category (hlktech.net, `id=product&cate=10Gradar`, fetched 2026-10-05) lists four LD1040 items and no ADC SKU:

| Page | Title on the page | id |
|---|---|---|
| Module | Hi-link 10G radar sensing module HLK-LD1040 … **uart output with MCU** | 1347 |
| Module | Hi-link 10G radar sensing module HLK-LD1040C … **IFC output** | 1346 |
| Kit | same two titles, testing kits | 1394, 1395 |

The comparison graphic on those pages (`res/_cache/auto/21/2119.png`) says **HLK-LD1040: with MCU** and **HLK-LD1040C: without MCU, support IFC communication configuration parameters**. It does not mention LD1040ADC. "IFC" is not defined as bytes on the page.

Product photos on the same pages, not a bench photo of this unit:

- LD1040 graphic silk reads **HLK-LD1040-V1.0**, three holes labelled VCC, OUT, GND.
- LD1040C photo (`res/_cache/auto/21/2124.png`) legend ends **040C V1.1**. Holes are labelled VCC, GND, TX/SDA, RX/SCL, OUT. It does not read LD1040ADC.

Searches of hlktech.com, hlktech.net, and the public web for `LD1040ADC` / `HLK-LD1040ADC` returned no Hi-Link page or manual with that model.

## Sibling documents (not an ADC manual)

These describe other names. They are cited so they are not silently reused.

**1. "Radar Sensor Module HLK-LD1040 Datasheet"**
Shenzhen Hi-Link Electronic Co., Ltd. Cover has **no version and no revision date**. PDF metadata: author zyt, CreationDate 2025-04-18. Reseller mirror of that file, fetched 2026-10-05. Not committed.

| Fact | Value | Where |
|---|---|---|
| Part named | HLK-LD1040 only | cover |
| P1 | V = DC 5–12 V, O = TTL high/low, G = ground | Table 6 |
| P2 | "Program download port" — no frame | Table 6 |
| O behaviour | high 1 s at power-on, then low through a 7 s wait; or three high/low pulses then sensing. Delay default 5 s, block default 2 s | startup notes |
| Vout | 3.2–3.4 V (logic level, not a sample) | Table 2 |
| Distance | a sensing-radius spec (2–4 m at 3 m hang), not a reported measurement | Table 4 |
| ADC / UART frame / track | absent | whole 8-page file |

**2. "Radar Sensor Module HLK-LD1040C Datasheet"**
Same company. Cover has **no version and no revision date**. PDF metadata: author zyt, CreationDate 2025-05-13. Different reseller file from (1). Not committed.

| Fact | Value | Where |
|---|---|---|
| Part named | HLK-LD1040C only | cover |
| P1 | VCC (5–12 V, 3.3 V custom), GND, TX, RX, OUT = TTL high/low | Table 6 |
| P2 | program/test point, "not for external use" | Table 6 |
| O behaviour | same high/low timing as the LD1040 datasheet | startup notes |
| Later section | "Mobile and Hand Scan Inductive Communication Protocol" | after the dimension page |
| Frame | header `0x3C 0x3A`, tail `0x3A 0x3E`, length byte, big-endian multi-byte fields | that section |
| Reported quantities | radar threshold, light threshold, delay, module id, output mode (O-port or PWM), light AD, **IF DC-voltage AD**, noise SUM0, signal SUM2 | field table and notes 1–9 |
| Distance or point | not in the field table | that section |
| Baud | not stated | that section |

Note 6 calls the mid-frequency byte an IF DC voltage ("AD value / 4", "byte/128 is the DC voltage"). That is a voltage used for engineering debug, not a range and not a track point. The section never uses the model string LD1040ADC.

A Hi-Link Q&A answer (ask.hlktech.com question 276866, posted 2025-10-02, answered 2025-10-10) says the C board's SDA/SCL silks are UART, not I²C, and names 9600 baud. That is a forum answer, not a manual, and it does not name LD1040ADC.

## What the ADC variant outputs

**Unknown.** No primary document states what HLK-LD1040ADC drives on the wire. The sibling manuals above are not that document.

## UNKNOWNs

- Whether a board silkscreened HLK-LD1040ADC V1.1 is a distinct SKU, a misread of HLK-LD1040C V1.1, or an unlisted PCB legend. The official C photo does not show the letters ADC.
- What "IFC" means, and why the product page says the C variant is "without MCU" while its datasheet prints a UART command set.
- Baud, logic level of TX/RX, and which pin is the ADC variant's output. Not in a document that names this part.
- No distance and no point that maps to `radar_range` or `radar_track_point`. No spatial-evidence adapter, because there is no cog.
- Not measured on hardware.
