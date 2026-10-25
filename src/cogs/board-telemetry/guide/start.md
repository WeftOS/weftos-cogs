# Start here

> Watch a board's power draw, fan and temperatures from the board itself, with no wiring.

This cog reads the monitors a Linux board already has and reports them once per window: input power, voltage and current from an INA2xx power monitor, fan RPM, the hottest thermal zone overall and per group (CPU, GPU, NPU, camera, DDR, other), the 1-minute load and free memory.

It opens no bus and needs no hardware change. If the kernel already shows the monitors under `/sys`, the cog can read them.

## The short path

1. **Check what the kernel exposes.** `ls /sys/class/hwmon /sys/class/thermal`. → see **What it reads**
2. **Install and run the cog.** `cog-board-telemetry --once` prints one report. → see **Set up**
3. **Read the `missing` list.** It names every source this board does not have. A Pi has no INA2xx or fan controller by default; that is normal, not a fault.
4. **Wire the vector to a consumer.** The 8 floats go to the store as id 28. → see **API**

You can test with no board at all: `--simulate` makes synthetic values marked `SYNTHETIC`, and `--sysfs-root DIR` reads a copy of a sysfs tree.

## How the signal flows

```diagram
flow
```

## Honest limits

- It reports what the kernel reports. A driver that is not loaded is an absent source, not a zero.
- Temperatures are the maximum over the window; power, voltage, current, fan speed and load are the mean.
- Only the first INA2xx (lowest hwmon number) feeds the headline power. All of them are listed in `power_monitors`.
- Not yet verified on hardware: the layout is tested against fixture trees shaped like a Ventuno Q and a Raspberry Pi, not against a live board.
