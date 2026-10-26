# Troubleshoot

> What each failure looks like and what to do.

## `unusable_frames` grows

`last_error` says why: `no blob handle in the reading` means `camera` runs with **No blob store** or without a broker; `the frame's blob has expired` means the model took longer than the frame lifetime. Raise the camera's **Frame lifetime**.

## `failures` grows

| `last_error` starts with | Meaning | Do this |
|---|---|---|
| `unavailable` | nobody serves `svc.infer/<service>` | place `npu-infer` |
| `service_error` | the service refused the request, for example it does not take `image_blob` yet or the model is unknown | the message says |
| `timeout` | queue or model load took longer than **Call timeout** | raise it |
| `unauthorized` | the cog was placed without its `svc.infer` input | place the package again |

## `unparsed_answers` grows

The model replied in prose. Try a larger model, or read `last_error` for the start of what it said.

## Boxes are all `[0, 0, 1, 1]`

The model gives labels without boxes (`labels_only_boxes` is high). That is the model's limit; see the limits on the first page.

## No frames at all

`frames_seen` is zero: **Camera source** does not match the camera's source id, or the camera is not publishing.
