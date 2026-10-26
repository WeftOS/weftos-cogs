# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

The report's `error` says why.

| `error` starts with | Meaning | Do this |
|---|---|---|
| `no video node in WEFTOS_DEVICES` | the host handed no video node | place the cog through the controller, or pass `--device` |
| `no handed capture node matches` | the camera name matches none of the handed nodes | check `/sys/class/video4linux/*/name` and the **Camera name** setting |
| `gst-launch-1.0: ...` | GStreamer is missing | install the GStreamer tools and base and good plugins |
| `gst-launch-1.0 exit status: 1` | the pipeline failed | run the same pipeline by hand: `gst-launch-1.0 v4l2src device=/dev/video0 num-buffers=2 ! image/jpeg,width=1280,height=720 ! fakesink` and read its message |
| `the camera delivered no frames in time` | the node is busy or not a capture node | another process holds the camera; check `fuser /dev/video0` |

## Frames per second is below the setting

The camera lowers its rate in low light (auto exposure) or the USB port is shared. Try a smaller size.

## `bus.connected` is false

`bus.error` holds the broker's refusal. `unauthorized` or `unknown_topic` means the cog was placed without its `[[outputs]]`; place the package again.

## A reading's hash is not on disk

The ttl passed. Raise **Frame lifetime** or fetch sooner.
