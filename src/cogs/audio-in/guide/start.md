# Start here

> Make the board's microphone available to every other cog, without any of them opening it.

`audio-in` is the one owner of the capture device. It reads 16 kHz mono audio and publishes it as the cogbus stream `audio.pcm/<source>` in 20 ms chunks (640 bytes of signed 16-bit little-endian samples). A speech, wake-word or level cog subscribes to that stream.

## The short path

1. **See which microphones the board has.** `cat /proc/asound/cards`. → see **The device**
2. **Run it once.** `cog-audio-in --once --device plughw:1,0` captures one second and prints a report. → see **Set up**
3. **Read the level.** `rms_dbfs` near -60 is a quiet room; speech is around -30 to -15. `-120` means silence from the driver, not a quiet room.
4. **Install it as a placed cog.** The host hands it the capture node and nothing else. → see **Set up**

You can test with no microphone: `--once --simulate` publishes a tone marked `SYNTHETIC`, and `--replay FILE` plays a recorded file through the same path.

## How the signal flows

```diagram
flow
```

## Honest limits

- It publishes what ALSA delivers. A card that cannot do 16 kHz mono natively is converted by ALSA's `plughw` layer.
- The stream is a ring in shared memory. A reader that falls behind loses chunks (the `seq` jumps); the writer never waits. A `block` reader is treated as `drop-oldest` by this writer.
- On a node where every cog runs under one user id, the node refuses a second claim on the microphone but cannot stop a cog from opening the device behind its back. The listing says `device_isolation: advisory`.
