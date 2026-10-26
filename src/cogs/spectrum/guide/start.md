# Start here

> Turn the radio's samples into a spectrum and a busy-or-quiet figure per band, and say how much was lost on the way.

`spectrum` reads the IQ stream that `sdr-pluto-rx` publishes. It never opens the radio. Every couple of seconds it publishes a power spectrum (`spectrum.psd`), and every minute a band occupancy reading (`band.occupancy`) for each band the radio visited.

## The short path

1. **Start the owner.** `sdr-pluto-rx` must be running and streaming. → see **Set up**
2. **Start this cog.** It subscribes to the owner's stream and waits for it if it is not up yet.
3. **Read the occupancy.** `occ.overall` is the share of time-frequency cells more than 10 dB above the noise floor. → see **Readings**
4. **Read the loss.** `loss.fraction` and `samples.lost` say how much of the stream never reached the spectrum. → see **Troubleshoot**

You can try it with no radio: `--once --simulate` makes a synthetic band marked `SYNTHETIC`, and `--once --replay FILE` analyses a capture made with the owner's `--record`.

## How the signal flows

```diagram
flow
```

## Honest limits

- Occupancy is relative to the noise floor of each frame, not an absolute level. A band that is wall to wall busy has no floor to measure against and reads low.
- Levels are dBFS of the 12-bit converter, not dBm. They are uncalibrated outside 325 to 3800 MHz and are marked so.
- Samples the owner lost, samples this cog missed, and samples from a dead receive path are counted apart and left out of the spectrum. A spectrum built from a stream that lost 30 percent says so in the same message.
