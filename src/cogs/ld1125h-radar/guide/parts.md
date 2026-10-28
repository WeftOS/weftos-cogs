# Parts

> The module, a ground wire, a 5 V wire, and one data wire from UTX into the Pi's RX. URX stays open.

| Part | Notes |
|---|---|
| HLK-LD1125H-24G | Blue PCB, about 31 × 16 mm on the owner's board. Four gold patches. Silk `90-00074`. Pins along the long edge, silk `5V G UR UT`. Board size and silk are from that board, not from a dimensioned drawing in manual V1.2 |
| Jumpers | Three. Red for 5 V, black for ground, green for data is a convention only |
| Seed header | Pi pin 2 (5 V), pin 6 (GND), pin 10 (RXD). See **Wiring** |
| 3.3 V USB-serial adapter | Preferred while `/dev/serial0` is the login console. Adapter TX listens to module UTX. Adapter RX stays off the module |

Manual pin names are VCC, GND, URX, UTX, 2 mm pitch. Silk `5V` is VCC, `G` is GND, `UR` is URX, `UT` is UTX.

The UART output is 3.3 V. Do not feed 5 V into a Pi GPIO pin. Power for the module may be 5 V on VCC; that is not the same net as the data pin.

No FCC or CE mark is stated in this manual.
