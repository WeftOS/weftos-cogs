# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-wake-word --once --simulate                         # a canned conversation
cog-wake-word --once --replay transcripts.jsonl         # lines {"text": ..., "start_ns": ..., "end_ns": ...}
cog-wake-word                                           # continuous; gates transcript/asr0 from the broker
```

Each command is printed as `{"command": ..., "from": ...}` and, with a broker, published.

## Placed on a node

Place `wake-word` in the same project as `asr`. Its `cog.toml` declares the input `transcript` and the outputs `transcript` and `telemetry`. It declares no device and runs no model.
