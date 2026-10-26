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

Lower the capture gain with `amixer`.

## `bus.connected` is false

`bus.error` holds the broker's refusal. `unauthorized` or `unknown_topic` means the cog was placed without its `[[outputs]]`; place the package again.

## Chunks per second below 50

The reader of the pipe is starved. Look for a busy CPU; `short_reads` counts failed reads.
