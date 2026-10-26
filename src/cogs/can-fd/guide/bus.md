# The bus

> What the numbers in the report mean.

## Controller state

`controller.state` is the kernel's: `ERROR-ACTIVE` (normal), `ERROR-WARNING`, `ERROR-PASSIVE` (the error counters passed 127), `BUS-OFF` (passed 255; the controller has left the bus) or `STOPPED` (the interface is down). A virtual interface has none.

`rx_error_counter` and `tx_error_counter` are the controller's own counters. A healthy bus keeps both near zero; a rising receive counter with no transmitter of ours usually means the wrong bit rate, a missing termination, or a stub that is too long.

## Error frames

The kernel reports controller and bus errors as error frames. The cog counts them by class in `error_classes`: `controller`, `protocol`, `transceiver`, `no_ack`, `bus_error`, `lost_arbitration`, `tx_timeout`, `bus_off`, `restarted`. `no_ack` seen by a listen-only node with another sender is normal for a bus with one transmitter and no other receiver.

## Bit rate

`configured_bitrate` is what you told the cog. `controller.bitrate` is what the kernel runs. `bitrate_matches` is false when they differ. A listener at the wrong rate sees only errors.

## Load

`bus_load_pct` counts the bits of the frames seen (47 for a standard header, 67 for an extended, plus 8 per data byte), adds 20% for bit stuffing, and divides by what the bit rate carries in the window. Above 70% sustained is a busy bus.

## CAN FD

Frames up to 64 bytes are published with their length code (`dlc` 0 to 15). The load figure counts FD frames at the nominal rate, which overstates the load of a frame sent with bit-rate switching.
