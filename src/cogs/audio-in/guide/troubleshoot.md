# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

The report's `error` says why.

| `error` starts with | Meaning | Do this |
|---|---|---|
| `no capture node in WEFTOS_DEVICES` | the host handed no capture node | place the cog through the controller, or pass `--device` |
| `no handed capture card matches` | the card name matches none of the handed cards | check `/proc/asound/cards` and the **Card name** setting |
| `arecord: ...` | `arecord` is missing | install alsa-utils |
| `the capture ended` | `arecord` exited | run `arecord -D plughw:1,0 -f S16_LE -r 16000 -c 1 -d 2 /tmp/t.wav` by hand and read its message |

## Level reads -120 dBFS

The device delivers digital silence: a muted capture control or the wrong card. Check `amixer -c 1 contents` and unmute capture.

## Level is high and `clipped_samples` grows

The report's `clipping` is true when more than 1% of chunks hold a clipped sample. The cog sets the card's capture level at start (**Capture level**, 50% by default) and applies **Software gain** (0 dB). Calibrated on the Ventuno's U20CAM with speech at about 1 m: 100% clipped 89 of 350 chunks; 70% peaked at -2.3 dBFS; 50% at -7 dBFS with no clipping; 20% at -14 dBFS. Lower **Capture level** for a louder room, raise it for a quiet or distant voice, or set it to `off` to leave the card alone. The setting stays on the card after the cog stops; restore it with `amixer -c <card> sset Mic 100%`.

## `bus.connected` is false

`bus.error` holds the broker's refusal. `unauthorized` or `unknown_topic` means the cog was placed without its `[[outputs]]`; place the package again.

## Chunks per second below 50

The reader of the pipe is starved. Look for a busy CPU; `short_reads` counts failed reads.
