# The sketch

> What runs on the MCU, how to flash it, and what `foreign-sketch` means.

## What it is

`sketch/weftos_probe/weftos_probe.ino` in this cog's directory. It registers two methods with the router bridge:

| Method | Returns |
|---|---|
| `weftos_ping` | 42, always |
| `weftos_calls` | a counter that goes up by one on every call |

The cog checks the first on every probe. Anything else means the sketch is not ours.

## Flashing it is an operator action

It replaces whatever sketch is on the MCU, including one that an App Lab app put there. Do it deliberately:

```
arduino-cli compile --fqbn arduino:zephyr:ventunoq sketch/weftos_probe
arduino-cli upload  --fqbn arduino:zephyr:ventunoq sketch/weftos_probe
```

The sketch survives a reboot and an App Lab restart. It does not survive starting an App Lab app that ships its own sketch: that app reflashes the MCU on start, and the cog then reports `mcu: foreign-sketch`. Use bricks without sketches (a brick-only App Lab app, or the brick's model server run directly) on a node where WeftOS owns the MCU.

## `foreign-sketch`

The router answered, but `weftos_ping` is not 42 or does not exist. Flash the sketch again.

## After a router update

`arduino-router` ships as an apt package. A protocol change would break this cog's msgpack-RPC; hold the package or rerun the probe after every board update.
