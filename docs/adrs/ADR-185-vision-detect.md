# ADR-185: vision-detect, objects in camera frames by blob handle

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `vision-detect` 0.1.0

## Context

The see loop (Ventuno cog plan 5.5) takes a camera frame to a list of objects. The brick spike ran YOLOX-nano on the NPU through an Edge Impulse runner (7 ms inference, 60 of 60 frames found the person), but `npu-infer` does not start those runners yet, so the node's only model service is `svc.infer` with language and vision-language models. A frame is 70 KiB of JPEG at 720p, over the 64 KiB body limit and over `infer.v1`'s 40 K-character inline image; WeftOS ADR-122 B5 (section 13) adds a blob store whose handles travel in readings and calls.

## Decision

1. **Frames by handle.** The `camera` cog (ADR-179) puts each JPEG in the blob store and carries the `blob.v1` handle in its `video.frame-ref.v1` reading (`blob`; the schema gains that optional field, and its `hash` must equal the frame hash). This cog reads those readings (`policy = "sample"`) and passes the handle, not the pixels, to the model in `infer.v1` as `image_blob`. The handle travelling in the call is what grants the provider read access; this cog never fetches the bytes.
2. **`image_blob` is a proposal.** `infer.v1` has no such field yet; the `npu-infer` owner (card venq-p4) adds image-by-handle input. The name and shape were sent to that lane; a stub provider that reads the handle, hashes the bytes and answers is the test double, and the cog is written so that only `infer_body` changes if the name does.
3. **The model's answer is a list.** The prompt asks for a JSON array of `{label, bbox [x, y, w, h] in fractions of the frame, score}`; the parser takes the first array in the reply (plain, in a code fence, after prose, or under `detections` / `objects`), skips arrays of numbers (reasoning), clamps boxes inside the frame, and turns an item with a label and no usable box into a whole-frame box. Missing scores are 1.0, meaning not measured. The report counts `labels_only_boxes`. An empty list is published as a frame with no objects; a reply with no list is `unparsed` and not published.
4. **Load control.** One frame is in flight; newer frames replace waiting ones; calls are at least 3 s apart (`--min-interval-ms`); priority 3 sits below the assistant's 7, so a spoken question does not wait for a frame. A frame whose blob has expired, or whose reading has no handle, is not sent and is counted.
5. **Output.** `detections.v1` on `detections/<name>` (default `vis0`): the frame's hash, the model alias and the boxes.
6. **Not a detector.** The boxes and scores of a vision-language model are claims, not measurements. The ADR for a real detector service (YOLOX through an EI runner owned by `npu-infer`) replaces the model call in this cog, not its inputs or outputs.
7. **`--simulate`** looks at one canned frame with a canned answer; `--replay FILE` takes frame-ref bodies as JSON lines to the real service.

## Consequences

- Detection latency is the model's: seconds per frame, more when it loads. The camera runs faster than this cog looks; the rest is dropped on purpose.
- Boxes are approximate and often absent with small models; consumers must treat a `[0, 0, 1, 1]` box as "somewhere in the frame".
- The frame lifetime (10 s default) bounds how long the model has to start; a long queue loses frames.
- `interfaces = ["none"]`; it declares `[[inputs]] svc.infer` and `video.frame-ref`.

## Verification

Unit tests cover the parser (plain, fenced, keyed and prose-wrapped lists, reasoning arrays skipped, label-only and out-of-range boxes, junk items, bounds), the request (handle in, no pixels, small), usable-frame checks (no handle, expired), an answered, failed, unparsed and empty-list frame, options, the report and vector. `tests/cli.rs` runs the binary with `--simulate`, no broker and a replayed frame without a handle. On the Ventuno, with the real camera and a stub provider on a broker with the blob store, six of six asked frames reached the provider as handles, were read back with matching hashes and became `detections.v1` readings (see the card). WeftOS `tests/blob.rs` covers handing a handle in a call.
