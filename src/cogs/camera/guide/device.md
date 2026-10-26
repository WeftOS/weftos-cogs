# The device

> Which video node is opened, and how to choose between two cameras.

The host hands the owner the video nodes it declared (`/dev/video*`) in the environment variable `WEFTOS_DEVICES`. A UVC camera shows two nodes: the video node and a metadata node. The cog picks the lowest node whose `/sys/class/video4linux/<node>/index` is `0`, which is the video node.

## Choosing between cameras

With several cameras (for example the board's own CSI cameras and a USB camera), set **Camera name** to part of the name in `/sys/class/video4linux/<node>/name` (matched without regard to case), such as `U20CAM`.

## What it asks the camera for

MJPEG at the width, height and frame rate in the settings (1280x720 at 2 fps by default). A camera that cannot do that combination makes the capture fail; the report says `no_source` with the pipeline's message. Try `v4l2-ctl -d /dev/video0 --list-formats-ext` for what it can do.

## By hand

`--device /dev/video0` opens a node you name, whether or not the host handed it. Use it for a bench run from a shell.

## Ownership

Only one cog owns the video nodes. Placing a second cog that declares `/dev/video*`, or whose arguments name a `/dev/video` node, is refused.
