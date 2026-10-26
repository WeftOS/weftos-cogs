# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

`no broker: WEFTOS_COGBUS is not set` when run from a shell without a broker; otherwise `bus.error` has the broker's refusal (`unauthorized` or `unknown_topic` means the cog was placed without its `[[inputs]]`).

## It never wakes

Read `transcripts_seen`. Zero: `asr` publishes nothing (check its guide) or the **Transcript source** does not match its **Name**. Many but no commands: look at `asr`'s `last_text`; the recognizer may hear the phrase differently. Use `--once --replay` with a file of what it heard to see the gate's verdict.

## It wakes on talk that was not for it

The phrase is too common. Use two words, or a rarer one.

## The command is missing its first words

Speech after a pause longer than `asr`'s hangover is a second utterance. Say the phrase and the command together, or rely on the armed window (a bare phrase, then the command).
