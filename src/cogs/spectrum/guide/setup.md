# Set up

> Run it after the owner, point it at the owner's source name.

```
cog-spectrum --once --simulate                  # synthetic band, prints one report
cog-spectrum --once --replay cap.iq             # analyse a capture from sdr-pluto-rx --record
cog-spectrum --source venq-pluto                # live, from the node's cogbus
cog-spectrum --ask-plan 2437e6:5e6:5000         # also ask the owner to include this band
```

Live mode needs `WEFTOS_COGBUS` and `WEFTOS_COGBUS_TOKEN` and a cog manifest that lists `iq.cs16` as an input. The source name is the owner's `--service` (default `venq-pluto`). The cog reconnects and resubscribes when the broker or the owner restarts.

`--ask-plan` sends a `plan` call to `svc.sdr` so the owner visits a band for this cog; with no owner plans of other consumers it simply runs that band. Otherwise the cog analyses whatever the owner is tuned to.

Windows are 2 s (`--psd-secs`), occupancy intervals 60 s (`--occ-secs`), the threshold 10 dB (`--thresh-db`). `/status`, `/raw` (the newest spectrum), `/guide` and `/healthz` are on `127.0.0.1:8064`.
