# The device

> Which capture card is opened, and how to choose between two.

The host hands the owner the capture nodes it declared (`/dev/snd/pcmC<card>D<device>c`) in the environment variable `WEFTOS_DEVICES`. The cog turns the one it picks into `plughw:<card>,<device>`.

## Choosing between cards

A Ventuno Q has the board codec (card 0) and, with a USB camera attached, the camera's microphone (card 1). With no setting the cog takes the lowest card. To choose the camera's microphone, set **Card name** to part of its name as `/proc/asound/cards` shows it (for example `U20CAM`, matched without regard to case).

## By hand

`--device plughw:1,0` opens a device you name, whether or not the host handed it. Use it for a bench run from a shell. A placed cog should rely on the handed devices so the node's ownership record is true.

## Ownership

Only one cog owns a capture node. Placing a second cog that declares the same node is refused. So is placing a cog whose arguments name a `/dev/snd/...` path this cog owns.
