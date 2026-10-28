# API

> One JSON object per window on stdout. Spatial lines, when enabled, are a different sink and a different schema.

## Stdout report

| Field | Meaning |
|---|---|
| `status` | `no_source`, `no_frames`, `mov`, or `occ` |
| `source` | Device description, or null when the tty did not open |
| `simulated` | True only under `--simulate` |
| `kind` | `mov` or `occ` for the last accepted line in the window, else null |
| `distance_m` | Metres from that line, else null. Never a carried-over 0 |
| `frames` | Accepted lines still in the window |
| `mov_lines`, `occ_lines` | Counts of those lines in the window |
| `undecoded_lines`, `rejected_lines`, `resync_bytes` | Parser counters for the window |
| `checksum_errors` | Always 0. The manual defines no checksum |
| `window_s` | The interval or the `--once` window |
| `export` | `http://127.0.0.1:8054` when the bind succeeded, else null |
| `error` | A sentence naming the device, baud, and next check, else null |
| `timestamp` | Unix seconds |

There is no `heart_rate`, `breathing_rate`, `present`, `person`, or `track` field. `mov` and `occ` stay on this object. They are not promoted.

With the spatial feature, a refusal adds `spatial` (`no_pose` or `no_region`) and `reasons`. A line that is being written sets `spatial` to `emitting`.

## Export on 127.0.0.1:8054

| Path | Body |
|---|---|
| `GET /status` | The latest stdout object, or `{"status":"starting"}` |
| `GET /raw` | Recent distances: `{source, simulated, samples:[{t_ms, distance_m}]}` |
| `GET /guide` | `{"toml","pages"}` for this guide |
| `GET /healthz` | `{"ok":true}` |
| `GET /spatial` | Only with `spatial-evidence`. `application/x-ndjson` of `radar_range` lines |

## radar_range

Schema `spatial.evidence.v1`, type `radar_range`, frame `room_enu`. `position` is the antenna from `--radar-pose`. `range_m` is the module's `dis`. `fov_deg` is `[44, 44]`, from the manual's ±22° half-power angle read as a full beam. `targets` is 1 for one accepted line, not a person id. `uncertainty_m` defaults to 1.0 because the manual gives no sigma; override with `--spatial-uncertainty-m`. Proof is `MEASURED` for a real frame and `SYNTHETIC` under `--simulate`. Neither is ground truth.

No pose, no region, or no accepted line: no evidence line.

This cog does not emit SenML.
