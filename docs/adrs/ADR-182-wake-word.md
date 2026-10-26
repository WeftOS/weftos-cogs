# ADR-182: wake-word, the transcript-confirmed wake gate

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `wake-word` 0.1.0

## Context

The brick spike found the on-device keyword model ("Hey Arduino", NPU, 25 ms per window) caught every wake phrase but also fired at full confidence on three ordinary sentences, and recommended using it only as a first stage with ASR confirming the following audio. The CPU recognizer already transcribes each utterance (ADR-181), so the second stage costs nothing extra and needs no NPU time.

## Decision

1. **The gate reads transcripts.** The cog subscribes to `transcript/<source>` (default `asr0`) and publishes `transcript/<name>` (default `wake0`) only for commands.
2. **Matching.** Words are lower-cased with punctuation stripped. The phrase (default "hey arduino") must appear in order and adjacent anywhere in the transcript. A phrase word of five letters or more may be one edit off (`arduin`); shorter words must be exact, so `hay` is not `hey`.
3. **The command** is the words after the phrase. A transcript that is only the phrase arms the gate for 8 s (`--listen-s`): the next transcript starting inside the window is the command, phrase or not, once. Everything else is ignored and counted.
4. **No keyword model yet.** The NPU keyword first stage is not wired; if it is added it comes before `asr` and only wakes the recognizer. The gate here stays as the confirming stage.
5. **Output shape.** A command is a `transcript.v1` reading with the times of the transcript it came from, `confidence` 1.0 (not measured).
6. **`--simulate`** runs five canned transcripts (two commands, one bare wake, two ignored); `--replay FILE` takes JSON lines.

## Consequences

- Wake latency is the recognizer's: the phrase is known only after the utterance ends and is transcribed.
- It is not a speaker check; a recording of the phrase wakes it.
- A single common word as the phrase would wake on every mention.
- `interfaces = ["none"]`; no device, no model.

## Verification

Unit tests cover the command after the phrase, mid-sentence phrases, near misses of long and short words, the armed window (once, expiry), another phrase, an empty phrase and odd input, the options, the canned conversation, replay parsing, the transcript body, the report and vector. `tests/cli.rs` runs the binary with `--simulate`, no broker and a replayed file with another phrase. The on-board chain run is in the card.
