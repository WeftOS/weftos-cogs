# Start here

> Make the board's camera available to every other cog, without any of them opening it.

`camera` is the one owner of the V4L2 device. It captures MJPEG frames and announces each as a `video.frame-ref.v1` reading on `video.frame-ref/<source>`: a BLAKE3 hash of the frame, its size, `mjpeg`, and how long the bytes are kept. A detector or a viewer cog reads the readings and fetches the bytes by hash.

## The short path

1. **See which cameras the board has.** `v4l2-ctl --list-devices`, or `ls /dev/video*`. → see **The device**
2. **Run it once.** `cog-camera --once --device /dev/video0` captures a few frames and prints a report. → see **Set up**
3. **Look at a frame.** Run it continuously and open `http://127.0.0.1:8065/frame.jpg` on the board.
4. **Install it as a placed cog.** The host hands it the video nodes and nothing else. → see **Set up**

You can test with no camera: `--once --simulate` announces frames with valid structure and no image, marked `SYNTHETIC`, and `--replay FILE.jpg` announces a real picture through the same path.

## How the signal flows

```diagram
flow
```

## Honest limits

- The pixels do not go in the reading. Each frame is also put in the node's blob store (WeftOS ADR-122 B5), and the reading carries its `blob.v1` handle in `blob`: a reader that receives the reading may fetch the JPEG or hand the handle to a service (a vision model) in a call. Without a broker, or with **No blob store** set, the reading has only the hash; the bytes are then reachable on this node's loopback, from the data directory or at `/frame.jpg` (the latest one).
- Frames are MJPEG as the camera makes them. Nothing is decoded here. H.264 on request is not implemented.
- A frame's bytes are deleted after `--ttl-ms` (10 s by default); a reading older than that points at nothing.
- On a node where every cog runs under one user id, the node refuses a second claim on the camera but cannot stop a cog from opening `/dev/video0` behind its back. The listing says `device_isolation: advisory`.
