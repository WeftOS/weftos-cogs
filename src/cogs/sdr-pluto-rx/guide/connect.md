# Connect the radio

> Right port, right power, an antenna on the receive port, and room to cool.

## Plug orientation

The Pluto has two USB-C ports.

- One goes to a CH340 USB-serial bridge. It is the Linux console and nothing else. If this is the only port plugged into the computer, the radio does not appear, although a port (`1a86:7523`) does.
- The other is the Zynq's own USB port, which carries libiio. It enumerates as `0456:b673` and brings the network gadget (`192.168.2.1`, RNDIS) on Linux.

Plug the computer into the Zynq port. Check with `lsusb | grep -i 0456` (Linux) or `system_profiler SPUSBDataType` (macOS). A port appearing is not proof: the bridge has its own power, so it enumerates with the radio dead.

## Power

The Pluto draws from its USB port. A laptop or board port that browns out ends the session. Put a charger or a powered hub on the other (console) port if you see disconnects; the Ventuno Q runs it that way and the board's power monitor shows no draw from its own port.

## Antenna

Use the receive port, RX port A (the cog selects it). The stock whip antennas work indoors for FM, ADS-B at 1090 MHz and 2.4 GHz Wi-Fi. Do not connect a transmitter, an amplifier output or a dummy load to the receive port without an attenuator: the input is not protected against more than about -10 dBm.

## Heat

The Zynq idles near 60 degrees C in open air and the AD9364 near 40. Both are in `hw.temp_zynq_c` and `hw.temp_phy_c`. Next to a busy board, give the Pluto airflow. A rising Zynq temperature with no change in load is the first sign to look at.

## Receive only

On start the cog switches digital loopback off, switches the DDS tones off and sets the transmit attenuation to its maximum (-89.75 dB), and records what it found in `safe_state`. A crashed earlier client can leave loopback on, which makes the receive path read zeros; the cog also treats an all-zero stream as a fault.
