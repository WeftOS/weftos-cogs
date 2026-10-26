# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

The report's `error` says why.

| `error` starts with | Meaning | Do this |
|---|---|---|
| `the recognizer worker did not become ready` | the model or Python is missing, or loading took over two minutes | check **The model**; run the worker by hand: `python asr_worker.py MODEL_DIR 3` |
| `<path>: No such file or directory` | the Python path is wrong | set **Python** |

## No transcripts although people are talking

- `noise_floor_dbfs` very high (above -35): a loud room or a hot microphone raises the bar. Lower the capture gain.
- `speaking` never true: raise sensitivity with a lower **Speech margin**, or lower **Speech floor**.
- `empty` grows: the recognizer heard noise, not speech. That is the second stage working.

## Transcripts are cut in the middle of a sentence

A pause longer than **Hangover** (600 ms) ends the utterance. Raise it.

## `skipped_chunks` grows

The recognizer was busy longer than the stream's ring holds (about 130 s) or the reader fell behind. Reduce the recognizer's load.

## `errors` grows

`last_error` holds the worker's message. The worker keeps serving after an error.
