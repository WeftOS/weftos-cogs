# ADR-179: camera, the node's V4L2 camera owner

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `camera` 0.1.0

## Context

Vision cogs (`vision-detect`, an assistant that describes a scene) need frames, and a V4L2 node serves one streaming client. The Ventuno cog plan (5.2) makes one cog the owner; the owner's USB camera (a UVC part, MJPEG) comes first, CSI later. Pixels are large: plan 4.3 keeps raw data where it is made and sends handles, and `video.frame-ref.v1` is the handle schema.

## Decision

1. **One owner.** `[[devices]] path = "/dev/video*"` (the UVC metadata node is a second node of the same camera). The node refuses a second claim and a foreign device argument.
2. **Capture node.** From `WEFTOS_DEVICES` the cog takes the lowest `/dev/videoN` whose `/sys/class/video4linux/videoN/index` is 0 (a UVC camera's second node has index 1), optionally filtered by `--camera-name` against the node's sysfs name. `--device` names a node by hand.
3. **Capture by GStreamer.** `gst-launch-1.0 v4l2src device=<node> ! image/jpeg,width=W,height=H,framerate=F/1 ! multifilesink max-files=16`, the camera's own MJPEG, no decoding. The board has GStreamer (the spike used it); V4L2 ioctls from Rust would be a second implementation to keep right. The cog polls the output directory; a file is complete once the next index exists. `--once` runs a bounded pipeline (`num-buffers`).
4. **Frames stay local.** Each frame is stored as `<blake3>.jpg` in the cog's data directory and deleted after `--ttl-ms` (10 s). The latest frame is served on the loopback export as `/frame.jpg`. Nothing carries pixels on the bus.
5. **The reading.** `video.frame-ref/<source>` (default `cam0`) gets one `video.frame-ref.v1` per frame: `hash`, `width`, `height` (read from the JPEG's start-of-frame marker), `pixel_format: mjpeg`, `ttl_ms`. Fetching the bytes by hash across processes waits for the node's blob store (WeftOS ADR-122 B5); on this node a consumer can read the data directory.
6. **Not done: H.264.** "On request, if small" is not implemented. An encoder pipeline is a second gst branch started by a service call; it is left until a consumer needs it.
7. **Fail honestly.** No handed video node, no GStreamer, a pipeline that exits or no frame within 10 s is `no_source` with the reason and an all-`-1` vector.
8. **`--simulate`** announces frames with valid structure (SOI, comment with a counter, SOF with the size, EOI) and no image data, so their hashes differ and nothing can decode them; `--replay FILE.jpg` announces a real picture.

## Consequences

- MJPEG at the requested size and rate must be offered by the camera; one that is not fails the pipeline and says so.
- One gst process per camera and a polling loop (30 ms) cost little CPU; encoding is the camera's.
- `interfaces = ["camera"]`; `blake3` is the only dependency beyond the shared ones (pure Rust).

## Verification

Unit tests cover capture-node choice (index 0, name filter, no sysfs), JPEG size parsing (baseline and progressive, junk), the frame store (hash name, ttl collection), the completeness rule of the capture directory, the gst pipeline arguments, the reading's shape, the report and vector, and a stored frame being served. `tests/cli.rs` runs the binary with `--simulate`, `--replay` and no device. The on-board run (live frames from the U20CAM, a second cog refused the camera) is in the card.
