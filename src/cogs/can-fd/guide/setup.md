# Set up

> Bring the interface up as the operator, then run the cog.

## Real hardware

The cog never does this; you do:

```
sudo ip link set can0 up type can bitrate 500000            # classic CAN
sudo ip link set can0 up type can bitrate 500000 dbitrate 2000000 fd on     # CAN FD
```

Add `listen-only on` to be sure the controller cannot acknowledge or send. Check with `ip -details link show can0`. Wire the bus through a transceiver, terminate **both** ends with 120 ohms, and keep stubs short.

## Without hardware (a virtual interface)

```
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
cog-can-fd --once --iface vcan0 --window 10
```

Undo it with `sudo ip link del vcan0` (and, if nothing else uses it, `sudo modprobe -r vcan`).

## From a shell

```
cog-can-fd --once --iface can0 --bitrate 500000      # listen ten seconds
cog-can-fd --iface can0 --bitrate 500000             # continuous; status on :8072
cog-can-fd --once --simulate                         # synthetic traffic
cog-can-fd --once --replay drive.log                 # a candump -L log
```

## Placed on a node

Place `can-fd` through the controller. Its `cog.toml` declares the interfaces (`[[devices]] netdev = "can*"`), the stream and the telemetry output. The host refuses the placement if another cog owns a CAN interface, and hands the owner the interface names in `WEFTOS_DEVICES`.
