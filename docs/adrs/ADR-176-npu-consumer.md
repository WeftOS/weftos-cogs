# ADR-176: npu-consumer, a test caller for the NPU service

**Status:** Proposed
**Date:** 2026-10-26
**Cog:** `npu-consumer` 0.1.0

## Context

`npu-infer` (ADR-175) answers `svc.infer/<node>` calls on the node's cogbus. Accepting it on a real node needs a placed cog that calls it through the real broker, with a real token, and a way to run two such callers for hours against different models. A throwaway broker (the `npu_demo` example in weftos) proves the cog, not the placed path.

## Decision

1. **One small caller.** `npu-consumer` declares `[[inputs]]` `svc.infer` (`infer.v1`) and `telemetry` (`telemetry.v1`), connects with `WEFTOS_COGBUS` / `WEFTOS_COGBUS_TOKEN`, and calls the service one request at a time, cycling `--models`. It opens no device (`interfaces = ["none"]`) and speaks the cogbus protocol itself, like `npu-infer`.
2. **Everything is logged.** Each call is one JSON line: model, priority, total / queue / load / run milliseconds, whether the call had to load the model, tokens, or the broker's error code. A `summary` line (calls, failures, loads, p50 / p95 / max total) is written every `--summary-secs` and at exit, so a killed run still has one. With `--watch` the service's telemetry (queue depth, resident models, memory, evictions, restarts) is logged every `--watch-every` seconds.
3. **Two instances are the two-consumer run.** Place it twice with different `--models`, `--priority` and `--role`. `--duration-secs` bounds a run; 0 runs until stopped.
4. **Survives restarts.** A lost connection is retried with backoff; `unavailable` and `busy` answers pause two seconds instead of spinning.
5. **Placement.** A placed cog's argument values may contain only `[A-Za-z0-9._:+/-]`, so `--models` takes `+` as well as `,` (`gemma3-1b+qwen3-4b`). A bare `--once` is the host's run-to-completion mode: the cog runs as configured and exits at `--calls` or `--duration-secs`. The client answers the broker's idle ping with a pong frame, so a long call does not look like a dead connection; a hello refused as `busy` (8 connections per instance) is retried with backoff.
6. **`--once --simulate`** prints one line marked `SYNTHETIC` and calls nothing, for the gate.

## Consequences

- It adds load by design. Place it only where an NPU run is wanted.
- Latency in the log includes queueing behind the other caller and any model load; that is the number a soak is after.
- It checks nothing about the answers' content.

## Verification

Unit tests cover the percentiles, option limits and telemetry extraction; `tests/cli.rs` runs the binary against a stand-in broker (model cycling, per-call log lines, a refusal counted and the run continuing).
