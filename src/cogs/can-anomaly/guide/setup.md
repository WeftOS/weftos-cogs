# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-can-anomaly --once --simulate                       # synthetic bus, virtual time
cog-can-anomaly --once --replay drive.log --learn-s 20  # a candump -L log
cog-can-anomaly --source can0                           # live, from the broker
```

Each event is printed as a JSON line and, when watching live, published. Replay and simulation only print.

## Placed on a node

Place `can-anomaly` in the same project as `can-fd`. Its `cog.toml` declares the input `can.frame` and the outputs `event` and `telemetry`. It declares no device.

## Choosing when to start

Start it with the bus in its normal state. Restarting it relearns, so a restart also clears a baseline that learned something abnormal.
