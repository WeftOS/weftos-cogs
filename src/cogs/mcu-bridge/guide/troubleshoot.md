# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

The report's `error` says why.

| `error` starts with | Meaning | Do this |
|---|---|---|
| `router unavailable: ...Permission denied` | the user is not in the `arduino-router` group | add the group, or run as that group (see **Set up**) |
| `router unavailable: ...No such file` | the router is not running | `systemctl status arduino-router` |
| `the router is there but the MCU did not answer` | the MCU is hung or in its bootloader | reset the board; reflash the sketch |

## `foreign_sketch`

Another sketch is on the MCU. See **The sketch**.

## A call comes back `method ... is not allowed`

The method is not in **Allowed methods**. Add it only if the sketch really has it.

## A call comes back with an error from the router

The text is the router's or the sketch's own. A wrong parameter count or type for the method is the usual cause.

## `bus.serving` is false

`bus.error` holds the broker's refusal. `writer_conflict` means another instance already serves `svc.mcu/<service>`; `unauthorized` means the cog was placed without its `[[outputs]]`.
