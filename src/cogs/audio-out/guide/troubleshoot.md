# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

The report's `error` says why.

| `error` starts with | Meaning | Do this |
|---|---|---|
| `no playback node in WEFTOS_DEVICES` | the host handed no playback node | place the cog through the controller, or pass `--device` |
| `no handed playback card matches` | the card name matches none of the handed cards | check `/proc/asound/cards` and the **Card name** setting |
| `aplay: ...` | `aplay` is missing | install alsa-utils |
| `no broker: WEFTOS_COGBUS is not set` | run from a shell without a broker | use `--replay` or `--simulate`, or place the cog |

## Silence but chunks are played

`chunks_played` grows and `rms_dbfs` is above -60 but nothing is heard: the output is muted or routed elsewhere. Check `amixer -c 0 contents` and unmute the playback control.

## `idle: true`

The stream has no writer yet, or the writer is quiet. Check that the cog that writes `audio.pcm/<source>` is running and uses the same source id.

## `dropped_chunks` grows

The cog was starved for a moment (a busy CPU) or the writer sent faster than real time. The cog plays on from the oldest record still safe to read.

## `bus.error` is set

The broker refused the subscription. `unauthorized` or `unknown_topic` means the cog was placed without its `[[inputs]]`; `cross_project` means the writer is in another project.
