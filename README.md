<div align="center">
<a href="https://weftos.weavelogic.ai/docs/weftos/platform/cogs">
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/WeftOS/.github/main/profile/assets/banner-dark.svg">
  <img src="https://raw.githubusercontent.com/WeftOS/.github/main/profile/assets/banner.svg" alt="WeftOS Cogs: signed hardware cogs for WeftOS hosts and Cognitum Seeds" width="100%">
</picture>
</a>

[![Release](https://img.shields.io/github/v/release/WeftOS/weftos-cogs?label=release&color=2563eb)](https://github.com/WeftOS/weftos-cogs/releases)
[![WeftOS](https://img.shields.io/github/v/release/WeftOS/weftos?label=weftos&color=0d9488)](https://github.com/WeftOS/weftos/releases)
[![Docs](https://img.shields.io/badge/docs-weftos.weavelogic.ai-2563eb)](https://weftos.weavelogic.ai/docs/weftos/platform/cogs)
</div>

# WeftOS Hardware Cogs

Open, signed, per-arch sensor and radar cogs for [Cognitum](https://cognitum.one) Seeds and
[WeftOS](https://github.com/WeftOS/weftos) hosts. A cog is a small native program that reads one piece of
hardware and runs on a node as a signed, governed workload; every released cog is verified against a pinned
WeaveLogic key before it is trusted. How the Cog Host runs them is documented on the
[cogs platform page](https://weftos.weavelogic.ai/docs/weftos/platform/cogs).

- 29 cogs in this repository (release v0.1.4)
- 795 modules and 312 chips in the hardware catalog
- Built for armv7 (Seed, Pi Zero 2 W) and aarch64 (Pi 5)

A **cog** is a small Rust program that reads one piece of hardware (a radar, a ToF
imager, an ECG front-end, a mic, an IMU) and emits a JSON line per window plus an
8-float vector into the Seed's vector store. Each cog ships `cog.toml` (typed config),
`src/`, a rendered `guide/` (ADR-104), and tests.

## How a cog works

![Sensor to vector: hardware feeds a cog, the cog emits a JSON line and an 8-float vector per window, and the vector goes to the Seed store or onto a cogbus topic.](docs/assets/readme/sensor-to-vector.svg)

The cog reads one device and windows the signal. For each window it emits a
JSON line and an 8-float vector. Released cogs can post the vector to the Seed
store's ingest endpoint (`/api/v1/store/ingest`); new cogs publish it on a
cogbus topic, which stays on the node until an operator exposes it. See the
[ingest bridge](docs/cogs/ingest-bridge.md) and the
[cogs platform page](https://weftos.weavelogic.ai/docs/weftos/platform/cogs).

## Cogs

| Cog | Sensor | Reads |
|---|---|---|
| `ld1040c-motion` | HLK-LD1040C | 10 GHz Doppler motion/occupancy |
| `ld2450-radar` | HLK-LD2450 | 24 GHz up-to-3-target position tracking |
| `ld6002-radar` | HLK-LD6002 | 60 GHz FMCW vitals (heart/breath) |
| `rd-03e` | Ai-Thinker RD-03E | 24 GHz presence + ranging |
| `hlk-as201` | HLK-AS201 | attitude / IMU |
| `sen0213-ecg` | DFRobot SEN0213 (AD8232) | single-lead ECG via ADS1115 |
| `sen0628-tof` | DFRobot SEN0628 (VL53L7CX) | 8×8 matrix time-of-flight |
| `sound-detect` | KY-038 mic | sound events via ADS1115 |
| `mentra-live` | Mentra Live glasses | companion presence/battery bridge |
| `bridge` | — | receives remote nodes' readings into the Seed store |
| `catalog` | — | the on-Seed hardware catalog app |

## Build

```sh
scripts/repo-gate.sh                # workspace, explorer, wasm, catalog, cog gate
scripts/cross-build.sh <cog-id>     # armv7 (Seed/Pi Zero 2 W) + aarch64 (Pi 5)
scripts/cross-build.sh              # all cogs
```
Artifacts land in `.cargo-target/dist/<id>/`: stripped `cog-<id>-arm`, `cog-<id>-arm64`, and `manifest.json` from `scripts/cog_manifest.py`.

## Install

Released cogs are signed (Ed25519, pinned WeaveLogic key) and published at the
Sensor Explorer, which serves a COG-008 `registry.json` and per-arch downloads.
Install on a WeftOS host via `cog-sources`, or on a Cognitum Seed via the
`cogrepo` cog — both verify the signature against the pinned key before trusting
the bytes. See COG-008 / COG-012 for the trust model.

### Release and signing

![Release and signing flow: cross-build, sign each binary with Ed25519, attach an SBOM and provenance, verify the set, publish registry.json, and installs check the pinned key.](docs/assets/readme/cog-release.svg)

`scripts/release-gate.sh --key <pem> --version <tag>` runs the steps in order. It
cross-builds, signs each binary with `weft-cog-repo sign` (which refuses a key
whose public half is not the pinned one), writes a CycloneDX SBOM per cog and
in-toto provenance, then checks the whole set with `scripts/release-verify.py`.
It never generates a key and never tags. The signing key stays outside the
repository; CI does not sign releases.

## Security

Report a vulnerability through the private advisory described in
[SECURITY.md](SECURITY.md). Contributing notes are in
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
