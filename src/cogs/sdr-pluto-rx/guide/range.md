# Frequency range and rates

> 70 to 6000 MHz measured, 325 to 3800 MHz rated; what each limit means.

| | Value |
|---|---|
| Declared range | 70 to 6000 MHz (the measured range, after the firmware's AD9364 setting) |
| Rated range | 325 to 3800 MHz (the AD9363's specification) |
| Sample rate | 0.521 to 61.44 MS/s on the chip; 5 MS/s continuous on the Ventuno Q |
| Bandwidth | 0.2 to 56 MHz |
| Gain | -3 to 71 dB manual, or the chip's slow-attack AGC |

Outside the rated range the cog still tunes, and marks every band it reports there `uncalibrated`: the noise floor is about 5 dB worse and there are spurs above 4 GHz. Use readings there for presence and shape, not for levels.

Below 2.083 MS/s the chip needs its decimating FIR; the cog loads one when you ask for a lower rate and switches it off again when you ask for a higher one.

A rate above 7.5 MS/s over USB 2.0 loses samples. The cog still runs it and reports the loss.
