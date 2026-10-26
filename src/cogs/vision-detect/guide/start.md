# Start here

> Say what is in front of the camera, without moving pixels around.

`vision-detect` reads the frame announcements of the `camera` cog. Each announcement carries a handle to the JPEG in the node's blob store. The cog passes that handle, not the picture, to the node's vision model, asks for a list of the objects it sees, and publishes the list as `detections/vis0` (`detections.v1`).

## The short path

1. **Run `camera`** with the blob store on (the default). → see its guide
2. **Have the inference service** serving a vision model. → see **The model**
3. **Try it without either.** `cog-vision-detect --once --simulate` looks at one canned frame and prints two canned objects.
4. **Place it** with `camera` and `npu-infer` in one project. → see **Set up**

## How the signal flows

```diagram
flow
```

## Honest limits

- The model describes; it does not measure. A vision-language model's boxes are approximate and its scores are its own claim, not a calibrated probability. When it gives a label with no usable box, the detection covers the whole frame (`[0, 0, 1, 1]`); the report counts those as `labels_only_boxes`. A detector model (YOLOX on the NPU) would give real boxes and scores; the service does not run one yet.
- A vision model takes seconds per frame, so the cog looks at about one frame every few seconds, whatever the camera's rate. Newer frames replace older waiting ones.
- The frame's bytes live in the blob store only for the camera's frame lifetime (10 s by default). A slow model on a busy NPU can miss it; the report counts `unusable_frames`.
- It needs the service to accept an image by handle (`image_blob` on `infer.v1`). Until `npu-infer` does, calls are refused and counted as `failures`.
