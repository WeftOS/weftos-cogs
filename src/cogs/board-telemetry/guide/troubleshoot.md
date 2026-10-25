# Troubleshoot

> What each status means and what to check.

**`no_source`.** None of an INA2xx power monitor, a fan controller or a thermal zone could be read.
- Run the commands in **Set up**. If they print nothing, the kernel exposes no monitors.
- Inside a container, `/sys` may be a stripped view. Mount the host's, or point `--sysfs-root` at a copy.
- A permissions error on `/sys/class/hwmon` shows as no source; run the cog as a user that can read it.

**`power` is in `missing` but the board has an INA2xx.** The driver is not bound. Check `dmesg | grep -i ina`.

**`zones_unreadable` is not zero.** Some thermal zones are disabled or report an impossible value. They are left out of every maximum on purpose.

**Fan RPM is 0.** That is what the controller reports. A stopped fan and a missing fan look different: a missing one is not listed at all.

**Nothing at `:8062`.** The export runs in continuous mode only (`--interval`), and binds loopback. Use `--api-bind` to change it.

**`status` is `simulate`.** `--simulate` is on. The values are synthetic and nothing is stored.
