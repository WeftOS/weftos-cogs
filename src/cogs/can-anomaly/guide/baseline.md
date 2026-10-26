# The baseline

> What is learned, what is compared, and what each event means.

## Learning

For **Learning time** after start, per id (a standard 11-bit id and an extended 29-bit id with the same number are different ids) the cog counts frames and notes the data lengths used. It publishes one `can.baseline` event when it finishes, saying how many ids and frames per second it learned.

## Watching

In windows of **Check window** (5 s), against the baseline:

| Event | Severity | When |
|---|---|---|
| `can.new_id` | warn | an id that was not learned sends, once per id |
| `can.dlc_change` | info | a known id uses a data length it never used, once per length |
| `can.id_missing` | warn | an id that sent 0.5 frames/s or more is silent for two windows or five of its periods, whichever is longer |
| `can.id_back` | info | that id sends again |
| `can.rate_high` | warn | an id runs above 3 times its baseline and 5 frames/s over it |
| `can.rate_low` | warn | an id of 2 frames/s or more runs under a quarter of its baseline but still sends |
| `can.flood` | alert | the whole bus runs above 3 times its baseline total and 50 frames/s over it |

A rate event for the same id does not repeat within **Repeat time** (30 s).

## Time

The clock is the frames' own timestamps, so a recorded log replays exactly as the bus did. On a quiet live bus the cog advances its clock from the wall clock, so silence is noticed.
