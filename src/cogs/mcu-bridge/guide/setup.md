# Set up

> Run it from a shell, then place it on the node.

## From a shell

```
cog-mcu-bridge --once                      # one probe, prints a report
cog-mcu-bridge --interval 5                # continuous, status on :8066, serves svc.mcu on the broker
```

The router socket is `root:arduino-router`, mode 0660. A shell user outside that group gets `Permission denied` and the report says `no_source`. Add the group for the cog's user and nothing else on the MCU side, or run the test as `sudo -u <user> -g arduino-router cog-mcu-bridge --once`.

With `WEFTOS_COGBUS` and `WEFTOS_COGBUS_TOKEN` set (the host sets them for a placed cog) the cog becomes the provider of `svc.mcu/<service>` and publishes a telemetry reading each interval.

## Placed on a node

Place `mcu-bridge` through the controller like any cog. Its `cog.toml` declares the socket (`[[devices]] socket = "/run/arduino-router.sock"`), the service and the telemetry output. The host refuses the placement if another cog owns the socket. Cogs that call the service declare `[[inputs]] topic = "svc.mcu"`, schema `mcu.call.v1`.

## Needs

- The WeftOS sketch on the MCU (see **The sketch**).
- The `arduino-router` group for the user the cog runs as.
