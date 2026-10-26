#!/usr/bin/env python3
"""Speech synthesis worker for the tts cog: Kokoro or Piper through sherpa-onnx, on the CPU.

stdin : {"id": N, "text": "...", "out": "/file.f32"}   one JSON object per line
stdout: {"ready": true, "rate": R} once the model is loaded and warm, then
        {"id": N, "rate": R, "samples": M, "ms": T}  (mono float32 samples written to "out")
        or {"id": N, "error": "..."}
argv  : ENGINE MODELS_ROOT THREADS SPEAKER_ID SPEED        ENGINE is kokoro or piper
"""
import json
import sys
import time

import numpy as np
import sherpa_onnx

engine, root, threads, sid, speed = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), float(sys.argv[5])
if engine == "piper":
    d = f"{root}/vits-piper-en_US-amy-medium"
    cfg = sherpa_onnx.OfflineTtsConfig(model=sherpa_onnx.OfflineTtsModelConfig(vits=sherpa_onnx.OfflineTtsVitsModelConfig(
        model=f"{d}/en_US-amy-medium.onnx", tokens=f"{d}/tokens.txt", data_dir=f"{d}/espeak-ng-data"), num_threads=threads))
elif engine == "kokoro":
    d = f"{root}/kokoro-en-v0_19"
    cfg = sherpa_onnx.OfflineTtsConfig(model=sherpa_onnx.OfflineTtsModelConfig(kokoro=sherpa_onnx.OfflineTtsKokoroModelConfig(
        model=f"{d}/model.onnx", voices=f"{d}/voices.bin", tokens=f"{d}/tokens.txt", data_dir=f"{d}/espeak-ng-data"), num_threads=threads))
else:
    print(json.dumps({"error": f"unknown engine {engine}"}), flush=True)
    sys.exit(2)
tts = sherpa_onnx.OfflineTts(cfg)
warm = tts.generate("Warm up.", sid=sid, speed=speed)
print(json.dumps({"ready": True, "rate": warm.sample_rate}), flush=True)
for line in sys.stdin:
    q = None
    try:
        q = json.loads(line)
        t = time.time()
        a = tts.generate(q["text"], sid=sid, speed=speed)
        s = np.asarray(a.samples, dtype=np.float32)
        s.tofile(q["out"])
        print(json.dumps({"id": q["id"], "rate": a.sample_rate, "samples": int(len(s)), "ms": int((time.time() - t) * 1000)}), flush=True)
    except Exception as e:  # report, keep serving
        print(json.dumps({"id": q.get("id") if isinstance(q, dict) else None, "error": str(e)}), flush=True)
