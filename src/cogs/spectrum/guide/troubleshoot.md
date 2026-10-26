# Troubleshoot

> The loss counters tell you where samples went.

| Symptom | Likely cause and fix |
|---|---|
| `waiting`, no records | The owner is not running or not streaming. Start `sdr-pluto-rx` and check its `/healthz`. |
| `samples.lost` rising | The owner lost samples on the radio link (USB 2.0 tops out near 7.5 MS/s). Lower the rate. |
| `samples.skipped` rising | This cog fell behind the ring and records were overwritten. Check the load; the FFT needs about a tenth of a core at 5 MS/s. |
| `samples.dead` rising | The receive path delivers zeros. See the owner's guide: loopback or a loose cable. |
| occupancy reads 0 on a busy band | There is no quiet part to measure the floor against. Occupancy is relative; use the spectrum and `peak`. |
| `uncalibrated` is 1 | The band is outside the chip's rated 325 to 3800 MHz. Shapes are right, levels are not. |
