# Safety

> Not a medical device. Not a safety interlock. A distance line is not a person and not a cleared room.

`medical` is false in `guide.toml`. Breathing induction is how the module looks for a still body. This cog does not compute a breathing rate or a heart rate, and it does not emit a vital-sign record.

Do not use `no_frames` as proof that a space is empty. The manual says the module stops sending when it detects nothing. A disconnected UTX looks the same.

Do not use `mov` or `occ` as identification of a person. The words are the module's detection class for one approximate distance.

RF: the manual states 23.5–24.5 GHz. This guide does not add a regulatory approval the manual does not state.

Power: VCC may be 5 V. The data pin is 3.3 V UART. Do not connect pin 2 or pin 4 to a Pi GPIO pin. Do not connect Pi TX (pin 8) to URX while a getty owns the UART.

The export listens on loopback only. Do not publish port 8054 onto a LAN.
