# The model

> What runs the recognition, and where it lives.

The cog starts one worker process and talks to it in JSON lines. The worker is a short Python script carried inside the cog (`worker/asr_worker.py`); it loads the **Parakeet TDT 0.6B v2** model, int8-quantised, through **sherpa-onnx** and decodes each utterance on the CPU.

| Needs | Default |
|---|---|
| Python with `sherpa_onnx` and `numpy` | `$ASR_PYTHON`, else `/data/bench/venv/bin/python` |
| Model directory with `encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`, `tokens.txt` | `/data/bench/models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8` |

Set **Python** and **Model directory** in the settings for another location. Start-up takes a few seconds while the model loads; the report says `no_source` until it has.

The worker writes nothing; the cog saves each utterance to a small file in its data directory (four files, reused) and sends the path.
