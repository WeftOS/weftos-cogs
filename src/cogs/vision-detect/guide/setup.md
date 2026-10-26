# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-vision-detect --once --simulate                     # one canned frame, canned answer
cog-vision-detect --once --replay frames.jsonl          # frame-ref bodies to the real service
cog-vision-detect                                       # continuous; watches video.frame-ref/cam0
```

Each frame looked at is printed as `{"frame": ..., "labels": [...]}` and, with a broker, published as a detection.

## Placed on a node

Place `vision-detect` in the same project as `camera` and `npu-infer`. Its `cog.toml` declares the inputs `video.frame-ref` and `svc.infer` and the outputs `detections` and `telemetry`. It declares no device.
