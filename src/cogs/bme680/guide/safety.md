# Safety

> Not a safety device, not a gas alarm, and not a medical sensor. 3.3 V only.

The BME680's gas output is a resistance that moves when many volatile compounds are present. It does not tell you whether a room is safe to enter, whether a heater is leaking a specific gas, or what the carbon dioxide concentration is. Do not use this cog to permit or deny occupancy, ventilation shutdown, or breathing decisions.

Bosch documents an internal hot plate, typically 200 °C to 400 °C, under the metal lid. This cog's default target is 300 °C for 100 ms. That temperature is inside the package. It is not a reason to run the board from 5 V, and it is not a surface you should treat as a heater you can touch for warmth. Follow the datasheet's absolute maximums: supply pins −0.3 V to 4.25 V.

The Pi header's 5 V pins (pin 2 and pin 4) must not feed this breakout until someone has confirmed a regulator on this purple board. This lane could not see one. The GPIO pins are not 5 V tolerant. A 5 V I2C pull-up on the breakout is the same hazard.

The export listens on 127.0.0.1 only. Gas resistance and humidity are still information about the place the Seed sits. Do not republish the export onto a LAN from this cog; it will refuse a non-loopback bind.

No claim here has been checked on the physical board.
