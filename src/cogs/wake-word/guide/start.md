# Start here

> Act only on what is said to the board, not on everything it hears.

`wake-word` reads the transcripts of `asr` and passes on a command only when it follows the wake phrase (default **hey arduino**). It publishes the command, without the phrase, on `transcript/wake0`. An assistant cog reads that topic and never sees the chatter.

## The short path

1. **Run `audio-in` and `asr`.** → see their guides
2. **Try the gate with no hardware.** `cog-wake-word --once --simulate` runs a canned conversation: two commands out of five transcripts.
3. **Place it.** It reads `transcript/asr0` and publishes `transcript/wake0`. → see **Set up**

## How the signal flows

```diagram
flow
```

## Honest limits

- It confirms on the transcript, so a command costs one full recognition first. The brick spike found that the on-device keyword model alone fires on ordinary speech; this is the second stage that makes it safe, and it needs no NPU time.
- Wake detection is as good as the recognizer: a phrase heard as something else is a missed wake.
- It is not a speaker check. Anyone's voice, or a speaker playing a recording, wakes it.
