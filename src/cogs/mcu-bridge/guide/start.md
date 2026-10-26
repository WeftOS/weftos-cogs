# Start here

> Let every other cog use the MCU, through one owner that checks what is running on it.

The Ventuno Q has an STM32 MCU behind a router (`arduino-router`) that passes calls between the Linux side and a sketch. `mcu-bridge` is the one cog that talks to the router. Other cogs call it as the service `svc.mcu/venq-mcu`: a method name and a few scalar parameters in, the sketch's return value out.

## The short path

1. **Flash the WeftOS sketch** (an operator action, with its own page). → see **The sketch**
2. **Probe it.** `cog-mcu-bridge --once` calls `weftos_ping` and prints the result: `mcu: ok` and `ping_ms` of a few milliseconds. → see **Set up**
3. **Place the cog.** The host hands it the router socket and nothing else. → see **Set up**
4. **Call it from another cog.** `{"method": "weftos_ping"}` answers `{"result": 42}`. → see **API**

You can test with no board: `--once --simulate` uses a built-in fake router marked `SYNTHETIC`.

## How the signal flows

```diagram
flow
```

## Honest limits

- The sketch on the board today answers two methods, `weftos_ping` and `weftos_calls`. The CAN, I2C, ADC, PWM and GPIO vocabulary of the plan is not in the sketch yet, so this cog forwards only what the sketch has; add a method to the allowlist when the sketch gains it.
- The router calls are synchronous and the MCU serialises them; a call takes 1.5 to 5 ms. The service answers one call at a time.
- A call that names a method outside the allowlist is refused here, before the router.
- On a node where every cog runs under one user id, the node refuses a second claim on the router socket but cannot stop a cog from connecting to it behind the node's back (the socket needs the `arduino-router` group; see **Set up**). The listing says `device_isolation: advisory`.
