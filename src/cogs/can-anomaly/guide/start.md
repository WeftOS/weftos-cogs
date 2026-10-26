# Start here

> Hear about a bus that changed without watching it.

`can-anomaly` reads the frames `can-fd` publishes. For its first 30 seconds it only learns which ids send and how fast. After that it tells you, as `event.v1` readings on `event/can-anomaly0`, when an id appears that was not there, a periodic id goes quiet, an id runs much faster or slower, or the bus is flooded.

## The short path

1. **Run `can-fd`** on the bus. → see its guide
2. **Start this cog while the bus is doing what is normal.** Whatever it sees in the first 30 seconds is the baseline. → see **The baseline**
3. **Try it without a bus.** `cog-can-anomaly --once --simulate` runs a synthetic bus with a new id, a silent id and a burst through the engine and prints the events.
4. **Place it** with `can-fd`. → see **Set up**

## How the signal flows

```diagram
flow
```

## Honest limits

- **Counts, not meaning.** It knows ids and rates, not what a message says. It does not read the data bytes, so a changed value in a known id passes unseen. Rates, ids and silence catch a node that was added, removed, stuck or babbling.
- **A baseline is what it saw.** A bus that was already faulty while learning is learned as normal; an id that sends only on events (a door, a diagnostic reply) may be silent while learning and then looks new when it speaks.
- **Slow ids are not judged for silence.** Only ids that sent at least 0.5 frames/s while learning can go missing; below 2 frames/s there is no low-rate event.
- **No decisions.** It reports; whoever reads the events decides.
