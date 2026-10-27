# Parts

> You need the radar, a common ground, a 3.3 V supply, and a UART that is not the Seed console.

- **HLK-LD6004**, header silkscreen V1.0 on the module drawing in datasheet V1.1. The header pins, in order, are `1 OUT`, `2 TX0`, `3 RX0`, `4 GND`, `5 3V3`.
- **A 3.3 V supply.** The vendor datasheet asks for 3.1 V to 3.5 V, at least 1 A, and ripple at most 50 mV. Those figures are the vendor's. This board has not been measured. The Seed's 3.3 V pin is the right voltage and is not a 1 A supply. Use a separate 3.3 V regulator that can deliver the vendor current, and share ground with the Seed.
- **USB-serial adapter at 3.3 V logic**, or the Seed UART after the owner has freed it. Default device path is `/dev/ttyUSB0`.
- **Jumper wires.** Signal wires only. No 5 V wire.

`OUT` (pin 1) is GPIO P20. The datasheet says it is high when someone is in the region, by default. It is not UART data. Do not connect it to a Seed UART pin.

The datasheet's RX0 description column also says `uart0_txd`, the same words it uses for TX0. Wire by the pin name: TX0 transmits, RX0 receives.

This cog does not use I2C. There is no address and no bus switch.
