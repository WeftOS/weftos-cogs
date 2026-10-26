# The voice

> Which engine speaks, and where its files live.

The cog starts one worker process and talks to it in JSON lines. The worker is a short Python script carried inside the cog (`worker/tts_worker.py`); it loads the engine through **sherpa-onnx** and returns mono float32 samples, which the cog resamples to 16 kHz and cuts into 640-byte chunks.

| Engine | Files under the models directory | Native rate |
|---|---|---|
| `kokoro` | `kokoro-en-v0_19/` (`model.onnx`, `voices.bin`, `tokens.txt`, `espeak-ng-data`) | 24 000 Hz |
| `piper` | `vits-piper-en_US-amy-medium/` (`en_US-amy-medium.onnx`, `tokens.txt`, `espeak-ng-data`) | 22 050 Hz |

Defaults: Python `/data/bench/venv-tts/bin/python` (or `$TTS_PYTHON`), models in `/data/bench/models`. **Speaker** picks a Kokoro voice by number; **Speed** is 0.5 to 2.0 times normal.

Start-up loads the model and speaks a warm-up sentence into nowhere; the report says `no_source` until then.

## What is spoken

The text with control characters turned to spaces, whitespace collapsed and a limit of 600 characters.
