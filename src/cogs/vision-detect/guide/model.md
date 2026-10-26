# The model

> What the vision model is asked, and what comes back.

The cog calls `svc.infer/<service>` (default `venq-npu`) with an `infer.v1` request:

- `model`: the **Model** alias (default `qwen3-vl-4b`), `priority` 3, up to **Answer length** tokens;
- `image_blob`: the frame's `blob.v1` handle, copied from the camera's reading. The handle travelling in the call is what lets the service read the frame;
- `system` and `text`: a fixed instruction.

## The instruction

> You are the vision system of a small embedded board. You are shown one camera frame. Reply with only a JSON array of the objects you can see. Each item is {"label": a short noun, "bbox": [x, y, w, h], "score": 0 to 1}. x, y, w, h are fractions of the frame width and height, from the top left corner. Give at most 10 items. If you see nothing clear, reply [].

## How the answer is read

The first JSON array in the reply is used (inside a code fence, after prose, or under a `detections` / `objects` key). Each item needs a `label`. A `bbox` of four numbers inside the frame is kept (clamped to stay inside); anything else makes the box the whole frame. A missing score is 1.0, meaning not measured. An empty list is a frame with no objects and is published as such; a reply with no list is counted as unparsed and nothing is published.
