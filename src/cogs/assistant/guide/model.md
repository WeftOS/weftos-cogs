# The model

> Which model answers, and what it is told.

The cog calls `svc.infer/<service>` (default `venq-npu`) with an `infer.v1` request: the **Model** alias, priority 7, up to **Answer length** tokens, a fixed system prompt and the prompt text.

## The system prompt

> You are a voice assistant running on a small embedded board. You hear a spoken command and your answer is read aloud. Answer in one or two short plain sentences. No lists, no markdown, no emoji. If you do not know, say so briefly.

## The prompt

With no remembered turns the prompt is the command alone, which is what a chat model answers best. With some, they come first as a short transcript:

```
Earlier in this conversation:
Q: <earlier command>
A: <earlier answer>

Now answer this: <this command>
```

## Choosing a model

Any alias the `npu-infer` catalog serves. `qwen3.5-0.8b` (default) answers fastest; `qwen3-4b` answers better and costs a load when it is not resident. A reasoning model's `<think>` text is removed before the answer is spoken.

## What is spoken

The answer is stripped of reasoning blocks, markdown marks and extra whitespace and capped at 400 characters, at a sentence end where there is one.
