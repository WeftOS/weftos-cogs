# Start here

> Make the board's speaker available to every other cog, without any of them opening it.

`audio-out` is the one owner of the playback device. It plays the cogbus stream `audio.pcm/<source>` (default source `tts0`): 16 kHz mono signed 16-bit little-endian PCM in 20 ms chunks (640 bytes). A speech-synthesis cog writes that stream; this cog plays it.

## The short path

1. **See which outputs the board has.** `aplay -l`. → see **The device**
2. **Play a test tone.** `cog-audio-out --once --window 2 --replay tone.wav --device plughw:0,1`. → see **Set up**
3. **Place the cog and a writer.** The host hands this cog the playback node and nothing else. → see **Set up**
4. **Read `dropped_chunks` and `latency_ms`.** Drops mean the writer outran the speaker or the cog fell behind.

You can test with no speaker: `--once --simulate` plays a tone into a null sink, marked `SYNTHETIC`.

## How the signal flows

```diagram
flow
```

## Honest limits

- The stream is a ring in shared memory. A reader that falls a ring behind loses chunks (the `seq` jumps); the cog counts them as `dropped_chunks` and plays on.
- Playback starts when the first chunk arrives. There is no jitter buffer beyond ALSA's own, so a writer that sends in bursts will be heard stutter.
- One writer per stream: the broker allows one writer for `audio.pcm/<source>`. Two writers that should both be heard need two sources and two audio-out instances, which the device ledger refuses (one owner per device). Mixing is not done here.
- On a node where every cog runs under one user id, the node refuses a second claim on the speaker but cannot stop a cog from opening the device behind its back. The listing says `device_isolation: advisory`.
