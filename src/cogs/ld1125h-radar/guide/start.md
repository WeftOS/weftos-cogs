# Start here

> Read Hi-Link HLK-LD1125H-24G ASCII distance lines on a Cognitum Seed. The module detects a still person by breathing micro-motion. This cog does not report a heart rate or a breathing rate.

The `ld1125h-radar` cog opens a 3.3 V UART, resyncs on text lines, and parses `mov, dis=<metres>` and `occ, dis=<metres>`. It never sends configuration. A quiet module is `no_frames`, not a measured empty room.

`mov` and `occ` are the module's words for a larger motion and for a still body or a small motion. They are not a person and not a track.

## The short path

1. **Get the parts.** The module, three jumpers, and either the Seed header or a 3.3 V USB-serial adapter. See **Parts**.
2. **Wire data one way.** Module UTX (silk `UT`) to Pi pin 10. Ground and 5 V as in **Wiring**. Leave URX unconnected.
3. **Try the software with no module.** `--once --simulate` feeds synthetic vendor-format lines through the real parser and labels the report simulated.
4. **Install on the Seed** and run `--once` against `/dev/ttyUSB0` or the header UART once the console is free. See **Set up**.
5. **Place it** with a clear view. See **Mounting**.

## How the data flows

```diagram
flow
```

The default build prints one JSON object per window on stdout and serves `GET /guide` on `127.0.0.1:8054`. A build with cargo feature `spatial-evidence` can also write `radar_range` lines, and only when `--spatial-out` is set. Those lines are a separate sink. They are not mixed into the stdout object.

## What you get

| Output | Where | Use |
|---|---|---|
| JSON report | stdout | `mov`, `occ`, or a named empty (`no_source`, `no_frames`), plus `distance_m` |
| Store vector, id 28 | `127.0.0.1:80` `/api/v1/store/ingest` | Only after an accepted line. Failure stays on stderr |
| Export | `http://127.0.0.1:8054` | `/status`, `/raw`, `/guide`, `/healthz` |

See **API** for every field. See **Safety** before treating a line as a person.
