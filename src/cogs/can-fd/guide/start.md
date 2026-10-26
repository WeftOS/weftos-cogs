# Start here

> Put the frames of a CAN bus where every other cog can read them, and never put one on it.

`can-fd` listens to one SocketCAN interface (`can0`, `vcan0`, ...). Each data frame, classic or CAN FD, becomes a `can.frame.v1` record in the stream `can.frame/<source>`. The report counts frames, error frames by class, bus-off events and the controller's error counters, estimates the bus load, and says whether the interface runs at the bit rate you configured.

## The short path

1. **Know your bus.** Bit rate, classic or FD, termination at both ends. → see **The bus**
2. **Bring the interface up** (an operator step): `sudo ip link set can0 up type can bitrate 500000`. → see **Set up**
3. **Run it.** `cog-can-fd --once --iface can0 --bitrate 500000` listens for ten seconds and prints a report.
4. **Place it.** The host hands it the interface name and nothing else. → see **Set up**

You can test with no bus and no root: `--once --simulate`, a `candump -L` log through `--replay`, or a virtual interface `vcan0`.

## How the signal flows

```diagram
flow
```

## Honest limits

- **Receive only.** The cog opens a raw socket and reads. It has no code that writes to the socket. Sending on a bus (actuation) is a separate decision, WeftOS ADR-123, which is not accepted.
- **It does not configure the interface.** A down interface is `no_source` with the reason. Setting a bit rate is the operator's, because a wrong rate can disturb a live bus.
- **Remote frames are counted, not published.** `can.frame.v1` has no place for them.
- **Timestamps are the wall clock when the frame was read**, not the controller's hardware stamp, so they carry scheduling jitter of a millisecond or so.
- **One interface per cog.** The cog claims every CAN interface on the node (`can*`) and reads one of them.
- The load figure is an estimate: bits of the frames seen, plus 20% for bit stuffing, over what the bit rate carries.
