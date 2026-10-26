# ADR-183: assistant, the answering step of the voice loop

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `assistant` 0.1.0

## Context

After the wake gate (ADR-182) a command needs an answer, and the node has one language model service, `npu-infer`'s `svc.infer/<service>` (ADR-175, WeftOS ADR-122 section 12). The NPU is shared with other work and scheduled by that service; this cog is a client like any other.

## Decision

1. **Commands in.** The cog subscribes to `transcript/<source>` (default `wake0`).
2. **One call per command** to `svc.infer/<service>` (default `venq-npu`), `infer.v1`: `--model` (default `qwen3.5-0.8b`, the smallest in the catalog, because a voice loop wants seconds), priority 7 (interactive), `--max-tokens` 96, a fixed system prompt that asks for one or two short plain sentences, and as text the command alone, or, with remembered turns (`--history`, default 2), a short "Earlier in this conversation: Q/A" transcript then "Now answer this: <command>" (the first on-board run with a `User:`/`Assistant:` text made the 0.8B model answer "No." to a plain question). A failed turn is not remembered.
3. **Cleaned for speech.** Reasoning blocks (`<think>...</think>`, an unclosed one to the end), markdown marks and extra whitespace are removed; the text is capped at 400 characters at a sentence end where there is one. An answer that is empty after cleaning counts as a failure.
4. **Never silent.** When the service cannot be reached, times out or refuses, the cog speaks "Sorry, I could not reach my language model." and counts the failure, keeping the service's message as `last_error`.
5. **Output.** The answer is a `transcript.v1` reading on `transcript/<name>` (default `reply0`), the shape the `tts` cog reads. There is no `speak` schema; `transcript.v1` is text with times, which is what a spoken line is.
6. **Timeouts.** The call waits `--timeout-s` (90) for queueing and a model load; the broker's limit is 10 minutes.
7. **`--simulate`** answers two canned commands with a canned reply; `--replay FILE` sends JSON-lines commands to the real service.

## Consequences

- The answer arrives after recognition, queueing and generation: seconds on a resident small model, ten or more when the model must load first.
- The cog speaks; it takes no actions. An action-taking assistant would need governance grants of its own.
- The model alias must be one `npu-infer` serves; the service's refusal lists them.
- `interfaces = ["none"]`; it declares `[[inputs]] svc.infer` with `infer.v1`.

## Verification

Unit tests cover prompt building and history (order, cap, failed turns), cleaning (think blocks, markdown, prefix, length), the `infer.v1` body against the schema's limits, an answered and a failed turn, the canned brain, the transcript body, the report and vector, and replay parsing. `tests/cli.rs` runs the binary with `--simulate`, no broker, and a replayed command with no service. The on-board run against the real `npu-infer` is in the card.
