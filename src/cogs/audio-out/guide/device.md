# The device

> Which playback card is opened, and how to choose between two.

The host hands the owner the playback nodes it declared (`/dev/snd/pcmC<card>D<device>p`) in the environment variable `WEFTOS_DEVICES`. The cog turns the one it picks into `plughw:<card>,<device>`.

## Choosing between cards

A Ventuno Q has the board codec (card 0, several devices) and, with a USB camera or sound card attached, a USB card. With no setting the cog takes the lowest card and device. To choose another, set **Card name** to part of its name as `/proc/asound/cards` shows it (matched without regard to case).

## By hand

`--device plughw:0,1` opens a device you name, whether or not the host handed it. Use it for a bench run from a shell.

## Ownership

Only one cog owns a playback node. Placing a second cog that declares the same node is refused. So is placing a cog whose arguments name a `/dev/snd/...` path this cog owns.
