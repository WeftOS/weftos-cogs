# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-asr --once --replay phrase.wav            # the file through the real recognizer
cog-asr --once --window 10                    # listen to the broker's mic stream for 10 s
cog-asr                                       # continuous; status on :8067
```

With `WEFTOS_COGBUS` and `WEFTOS_COGBUS_TOKEN` set (the host sets them for a placed cog) the cog subscribes to the microphone stream and publishes transcripts. Each heard utterance is also printed as a JSON line `{"heard": ...}`.

## Placed on a node

Place `asr` in the same project as `audio-in`. Its `cog.toml` declares the input `audio.pcm` and the outputs `transcript` and `telemetry`. It declares no device, so any number of listeners may read the same stream.

## Needs

- The model and Python described in **The model**.
- About 1 GB of memory while the model is loaded.
