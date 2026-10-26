# Start here

> One cog owns the PlutoSDR, streams its samples to the node, and says how many it lost.

`sdr-pluto-rx` opens the radio through libiio, puts it in a known receive-only state, and publishes what it hears as the `iq.cs16.v1` stream on the node's cogbus. Other cogs (spectrum, ADS-B, decoders) read that stream; none of them open the radio.

## The short path

1. **Plug the radio in the right way round.** The Pluto has two USB-C ports and only one of them is the radio. → see **Connect**
2. **Check the antenna and the heat.** → see **Connect**
3. **Run the cog.** `cog-sdr-pluto-rx --once` prints one report; without hardware it says `no_source`. → see **Set up**
4. **Read `samples`.** `delivered` against `expected`, and `lost`, tell you whether the link kept up. → see **Troubleshoot**

You can try everything with no radio: `--simulate` makes a synthetic signal marked `SYNTHETIC`, and `--replay FILE` plays back a capture made with `--record FILE`.

## How the signal flows

```diagram
flow
```

## Honest limits

- The data path is USB 2.0. About 7.5 MS/s is the ceiling, 5 MS/s is gap-free on the Ventuno Q; above that samples are lost, and the cog counts them.
- The 70 to 6000 MHz range is measured, not rated. The chip is rated 325 to 3800 MHz; outside that band levels are not calibrated.
- The stream carries no hardware timestamps. Loss is counted from the host clock; see **Troubleshoot** for what that can and cannot resolve.
- Receive only. The cog never enables a transmit buffer and refuses `tx_config`.
