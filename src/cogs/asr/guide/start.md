# Start here

> Turn the microphone stream into text that other cogs can read.

`asr` listens to `audio.pcm/<source>` (the `audio-in` cog's stream), cuts it into utterances, transcribes each and publishes a `transcript.v1` reading on `transcript/<name>` (default `asr0`). A wake-word or assistant cog subscribes to those transcripts.

## The short path

1. **Have a microphone stream.** Run `audio-in`. → see its guide
2. **Check the model.** The Parakeet files and a Python with sherpa-onnx must exist. → see **The model**
3. **Try it on a file.** `cog-asr --once --replay phrase.wav` prints the transcript. → see **Set up**
4. **Place it.** It reads the stream and publishes. → see **Set up**

You can test with no model: `--once --simulate` runs two synthetic tone bursts through the whole path and reports their length, marked `SYNTHETIC`.

## How the signal flows

```diagram
flow
```

## Honest limits

- Speech is found by energy, then confirmed by the transcript. The first second after start is spent learning the room, and nothing is heard in it.
- Utterances are transcribed one at a time on the CPU. With three threads Parakeet runs about 8 times faster than real time, so a 4 s phrase takes about half a second; a long utterance delays the next.
- Parakeet reports no confidence. Every transcript says `confidence: 1.0`, which means not measured.
- The model is English only.
