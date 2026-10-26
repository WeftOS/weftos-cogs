# Start here

> Give the board a voice that any cog can use, through one stream.

`tts` reads text from `transcript/<source>` (default `reply0`, the `assistant`'s answers), turns it into speech and writes it, in real time, to the stream `audio.pcm/<name>` (default `tts0`). The `audio-out` cog plays that stream.

## The short path

1. **Check the models.** Kokoro and Piper files and a Python with sherpa-onnx must exist. → see **The voice**
2. **Try it without a model.** `cog-tts --once --simulate` speaks a tone per word and reports the length.
3. **Speak a file of lines.** `cog-tts --once --replay lines.jsonl` with `{"text": "..."}` per line.
4. **Place it with `audio-out`.** Start `audio-out` with `--source tts0`. → see **Set up**

## How the signal flows

```diagram
flow
```

## Honest limits

- Synthesis runs on the CPU. Measured on the Ventuno at three threads: Piper makes speech in about a tenth of its length (a 5 s answer in half a second), Kokoro in about 1.1 times its length (a 4 s sentence in 4.7 s). Piper is the default for that reason; Kokoro is more natural but too slow for a conversation unless given more threads.
- Speech is written at real time, one 20 ms chunk each, so a long answer occupies the stream for its whole length; new text waits behind it (at most four waiting, then the oldest is dropped).
- There is no interrupting speech that has started.
- The voices are English.
