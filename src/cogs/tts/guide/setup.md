# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-tts --once --simulate                          # a tone per word
cog-tts --once --replay lines.jsonl --engine piper # lines {"text": "..."}, the real voice
cog-tts                                            # continuous; speaks transcript/reply0
```

With a broker (`WEFTOS_COGBUS`, `WEFTOS_COGBUS_TOKEN`) the speech goes to `audio.pcm/tts0`; without one the cog synthesizes and reports but has nowhere to write. Each spoken text is printed as `{"speaking": ..., "audio_ms": ...}`.

## Placed on a node

Place `tts` in the same project as `assistant` and `audio-out`. Its `cog.toml` declares the input `transcript` and the outputs `audio.pcm` (a stream) and `telemetry`. It declares no device; `audio-out` owns the speaker.
