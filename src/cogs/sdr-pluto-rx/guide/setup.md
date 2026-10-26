# Set up

> Install the cog, run it once, then let the node start it.

Needs libiio 0.19 to 0.26 (`apt install libiio0`; on macOS `brew install libiio`). The cog loads the library when it opens the radio, so a host without it still runs `--simulate`. On Linux a udev rule must give your user access to the radio (`0456:b673`) if you use `usb:`; the network gadget (`ip:192.168.2.1`) needs none.

```
cog-sdr-pluto-rx --once                         # one 3 s capture window, then the report
cog-sdr-pluto-rx --once --simulate              # no radio
cog-sdr-pluto-rx --band-plan 2437e6:5e6:2000,1090e6:2.4e6:1000,915e6:2e6:1000
cog-sdr-pluto-rx --once --record cap.iq         # keep the window as a capture file
cog-sdr-pluto-rx --once --replay cap.iq         # play it back
```

A band is `centre_hz:rate:dwell_ms[:bandwidth[:gain]]`. One band streams continuously; several take turns, one dwell each, and the report shows how long each plan ran.

The backend is chosen when the cog starts: it measures `ip:192.168.2.1` and `usb:` for about a second each and uses the faster (on the Ventuno Q, `ip:`). `--backend ip|usb|auto` and `--uri URI` override.

The stream goes to the node's cogbus when `WEFTOS_COGBUS` and `WEFTOS_COGBUS_TOKEN` are set. `GET /status`, `/raw`, `/guide` and `/healthz` are on `127.0.0.1:8063` (`--api-bind`).
