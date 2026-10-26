# Troubleshoot

> Read the counters before suspecting the radio.

| Symptom | Likely cause and fix |
|---|---|
| `no_source`, `libiio not found` | Install libiio, or set `LIBIIO_PATH` to the library. |
| `no_source`, no radio on the backend | Wrong USB port (see **Connect**), no power, or the radio is rebooting; the cog waits and reopens. |
| `dead_receive_path` | Every sample is zero. Digital loopback was left on, or the front end is not running. The cog switches loopback off when it takes the radio; if it persists, replug the radio. |
| `samples.lost` rising | The link cannot keep up. Lower the rate (5 MS/s is safe over USB 2.0), use `ip:` on Linux, or close other users of the USB bus. |
| `pending_unconfirmed` large, `lost` small | A loss is in progress and not yet confirmed (about 0.25 s). |
| `usb_hub.isochronous_active`, `rate_over_advice` | A camera or microphone streams on the same USB root hub and takes bandwidth from the radio; stay at `advised_max_rate_sps` (4 MS/s) or move one of them to another hub. |
| `disconnects` rising | The radio dropped off the bus: power or cable. The cog logs it and reopens; it never power-cycles anything. |
| `apply_errors` | A band outside the tuning range or a rate the chip refused; `error` says which. |

## What the loss count can and cannot tell you

The stream has no hardware timestamps and an overflow is silent. The cog counts samples against the host clock. A loss is declared when the shortfall stays above 20 ms of data for 0.25 s, so:

- a loss shorter than about 20 ms is indistinguishable from USB jitter;
- a loss rate under about 0.1 percent is indistinguishable from the crystal's drift;
- a gap is reported about 0.25 s after it happened, and the sample clock jumps then.

`samples.expected` and `samples.delivered` are the raw comparison and are always honest; `lost` is the part the cog is sure of.
