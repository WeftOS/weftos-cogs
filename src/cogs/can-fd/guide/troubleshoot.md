# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

| `error` starts with | Meaning | Do this |
|---|---|---|
| `the interface is down` | `ip link ... up` has not been done | bring it up as the operator (see **Set up**) |
| `the controller is bus-off` | the controller left the bus after errors | check bit rate, termination, wiring; restart the interface |
| `ip link show ...` | the interface does not exist | check the name; for a virtual one, create it |
| `socket(PF_CAN ...)` | the `can_raw` module is not loaded | `sudo modprobe can_raw` |
| `no CAN interface in WEFTOS_DEVICES` | the host handed no interface | place through the controller or pass `--iface` |

## Frames arrive but `error_frames` grows

Read `error_classes`. `bus_error` and `protocol` with a rising `rx_error_counter`: wrong bit rate or bad wiring. `no_ack` with a quiet bus: only one node on the bus, which cannot acknowledge itself.

## `bitrate_matches` is false

The interface runs at a different rate than **Configured bit rate**. Fix whichever is wrong; the cog changes neither.

## `kernel_rx_dropped` grows

The socket buffer overflowed: the cog was not scheduled in time, or the bus load is very high. Lower the other load on the node.

## No frames and no errors

The bus is silent, or the cog listens to the wrong interface. Check `iface` in the report and `candump` on the same interface.
