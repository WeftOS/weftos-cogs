# The phrase

> What counts as the wake phrase, and what happens after it.

## Matching

Case and punctuation are ignored. Each word of the phrase must appear in order and next to each other. A word of five letters or more may be one edit away (`arduin` for `arduino`); shorter words must be exact, so `hay arduino` is not `hey arduino`.

## After the phrase

- **Words after it** are the command: `Hey Arduino, what time is it?` publishes `what time is it`.
- **Nothing after it** arms the gate for **Armed time** (8 s by default). The next transcript, with or without the phrase, is the command. One command per arming.
- **The phrase mid-sentence** works: `well, hey arduino what time is it` publishes `what time is it`.

## Changing it

Set **Wake phrase** to any words, for example `computer`. A single common word wakes on every mention, so prefer two words.
