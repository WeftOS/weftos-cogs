# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-assistant --once --simulate                         # canned commands and answers
cog-assistant --once --replay commands.jsonl            # lines {"text": "..."} to the real service
cog-assistant                                           # continuous; answers transcript/wake0
```

Each answer is printed as `{"command", "reply", "ok"}` and, with a broker, published.

## Placed on a node

Place `assistant` in the same project as `wake-word` and `npu-infer`. Its `cog.toml` declares the inputs `transcript` and `svc.infer` and the outputs `transcript` and `telemetry`. It declares no device.
