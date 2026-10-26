#!/usr/bin/env python3
"""Speech recognizer worker for the asr cog: Parakeet TDT (sherpa-onnx, int8, CPU).

stdin : one JSON object per line, {"id": N, "path": "/file.raw"}  (16 kHz mono s16le)
stdout: {"ready": true} once the model is loaded, then {"id": N, "text": "...", "ms": M}
        or {"id": N, "error": "..."}.
argv  : MODEL_DIR THREADS
"""
import json
import sys
import time

import numpy as np
import sherpa_onnx

model, threads = sys.argv[1], int(sys.argv[2])
rec = sherpa_onnx.OfflineRecognizer.from_transducer(
    encoder=f"{model}/encoder.int8.onnx",
    decoder=f"{model}/decoder.int8.onnx",
    joiner=f"{model}/joiner.int8.onnx",
    tokens=f"{model}/tokens.txt",
    num_threads=threads,
    model_type="nemo_transducer",
)
print(json.dumps({"ready": True}), flush=True)
for line in sys.stdin:
    try:
        q = json.loads(line)
        a = np.fromfile(q["path"], dtype="<i2").astype("float32") / 32768.0
        t = time.time()
        s = rec.create_stream()
        s.accept_waveform(16000, a)
        rec.decode_stream(s)
        print(json.dumps({"id": q["id"], "text": s.result.text.strip(), "ms": int((time.time() - t) * 1000)}), flush=True)
    except Exception as e:  # report, keep serving
        print(json.dumps({"id": q.get("id") if isinstance(q, dict) else None, "error": str(e)}), flush=True)
