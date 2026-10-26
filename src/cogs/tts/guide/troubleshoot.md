# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

| `error` starts with | Meaning | Do this |
|---|---|---|
| `the synthesizer worker did not become ready` | the model or Python is missing, or loading took over two minutes | check **The voice**; run the worker by hand |
| `<path>: No such file or directory` | the Python path is wrong | set **Python** |
| `no broker: WEFTOS_COGBUS is not set` | run from a shell without a broker | use `--simulate` or `--replay`, or place the cog |

## Text arrives but nothing is heard

- `spoken` grows but `bus.stream_open` is false: no stream. Check `bus.error`.
- The stream is written but `audio-out` plays nothing: check that `audio-out` reads the same name (`--source tts0`).
- Check the speaker's volume and mute (`amixer`).

## `dropped` grows

Text arrives faster than it can be spoken. The assistant's answers are capped at 400 characters; lower **Answer length** there.

## `failures` grows

`last_error` holds the worker's message. The worker keeps serving after an error.
