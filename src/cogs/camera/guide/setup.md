# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-camera --once --device /dev/video0                  # a few frames, prints a report
cog-camera --device /dev/video0 --fps 5 --width 640 --height 480
```

With `WEFTOS_COGBUS` and `WEFTOS_COGBUS_TOKEN` set (the host sets them for a placed cog) the cog publishes one `video.frame-ref.v1` reading per frame. Without them it still captures, stores and reports; the report's `bus.connected` is false and `frame_refs_published` stays 0.

## Placed on a node

Place `camera` through the controller like any cog. Its `cog.toml` declares the device (`[[devices]] path = "/dev/video*"`) and the two outputs. The host refuses the placement if another cog owns a video node.

## Needs

- `gst-launch-1.0` with the `v4l2src` and `multifilesink` elements (GStreamer base and good plugins) on the board.
- Read and write access to the video node for the user the cog runs as (the `video` group on a stock image).
