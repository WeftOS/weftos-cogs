# Glossary

> Short names used by the manual, the report, and the spatial line.

| Term | Meaning |
|---|---|
| `mov` | Manual: a larger motion. Report status and `kind`. Not a person |
| `occ` | Manual: a still body or a small motion. Not a track |
| `dis` | Target distance in metres, approximate. Placeholder in the manual is `***` |
| breathing induction | The manual's name for the micro-motion method that sees a still person. Not a rate |
| `no_source` | The tty path did not open |
| `no_frames` | The tty opened and this window had no accepted line |
| `radar_range` | `spatial.evidence.v1` type for one distance along an unknown bearing inside the beam. Antenna position comes from the configured pose |
| `no_pose` | `--spatial-out` was set and `--radar-pose` was not. No evidence line is written |
| `source_id` | The installed object instance. Default `ld1125h-1`. Not the catalog module id and not a person |
| SYNTHETIC | Proof on lines produced by `--simulate`. Not a measurement |
| MEASURED | Proof on lines from an accepted UART frame. Still not ground truth: the manual calls the distance approximate, and this board is not yet tested |
| SenML | Not used. The manual does not define a non-spatial quantity for this cog to pack |
