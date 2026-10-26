# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-audio-in --once --device plughw:1,0          # one second, prints a report
cog-audio-in --interval 5 --device plughw:1,0    # continuous, status on :8063
```

With `WEFTOS_COGBUS` and `WEFTOS_COGBUS_TOKEN` set (the host sets them for a placed cog) the cog opens the stream `audio.pcm/<source>` on the node's broker and writes every chunk to it. Without them it still captures and reports; the report's `bus.connected` is false.

## Placed on a node

Place `audio-in` through the controller like any cog. Its `cog.toml` declares the device (`[[devices]] path = "/dev/snd/pcmC*D*c"`) and the two outputs. The host refuses the placement if another cog owns a capture node, hands the owner `WEFTOS_DEVICES`, and registers the stream topic with the broker.

## Needs

- `arecord` (alsa-utils) on the board.
- Read access to the capture node for the user the cog runs as (the `audio` group on a stock image).
