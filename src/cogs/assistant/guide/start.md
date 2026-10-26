# Start here

> Answer what was asked, out loud, with the language model the node already has.

`assistant` takes each command `wake-word` publishes, asks the node's inference service, and publishes the answer on `transcript/reply0`. The `tts` cog speaks that topic.

## The short path

1. **Have the loop's front half.** `audio-in`, `asr`, `wake-word`. → see their guides
2. **Have the inference service.** `npu-infer` must be placed and serving `svc.infer/venq-npu` with the model you choose. → see **The model**
3. **Try it without either.** `cog-assistant --once --simulate` answers two canned commands.
4. **Place it.** → see **Set up**

## How the signal flows

```diagram
flow
```

## Honest limits

- The answer arrives after the recognizer, the queue and the model. On the NPU a small model takes a few seconds; a model that is not loaded takes ten more. The loop is a conversation, not an instant reply.
- The NPU is shared. This cog calls at priority 7, above background work, and below nothing else it can control.
- It remembers the last two exchanges in the prompt only; nothing is stored.
- A language model can be wrong. Do not give it actions to take without a check; this cog only speaks.
