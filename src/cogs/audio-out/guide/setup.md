# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-audio-out --once --window 3 --replay phrase.wav --device plughw:0,1   # plays the file, prints a report
cog-audio-out --device plughw:0,1                                          # plays audio.pcm/tts0 from the broker
```

With `WEFTOS_COGBUS` and `WEFTOS_COGBUS_TOKEN` set (the host sets them for a placed cog) the cog subscribes to `audio.pcm/<source>` and plays every chunk. The writer is another cog that declares `[[outputs]] topic = "audio.pcm"`, class `stream`, schema `pcm.s16le.v1`, and opens the stream with source id `<source>`.

## Placed on a node

Place `audio-out` through the controller like any cog. Its `cog.toml` declares the device (`[[devices]] path = "/dev/snd/pcmC*D*p"`), the input and the telemetry output. The cog that writes the stream must be in the same project.

## Needs

- `aplay` (alsa-utils) on the board.
- Write access to the playback node for the user the cog runs as (the `audio` group on a stock image).
