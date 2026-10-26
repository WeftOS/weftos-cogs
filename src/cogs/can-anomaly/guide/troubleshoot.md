# Troubleshoot

> What each failure looks like and what to do.

## `no_source`

`no broker: WEFTOS_COGBUS is not set` from a shell without a broker; otherwise `bus.error` has the broker's refusal (`unauthorized` or `unknown_topic`: placed without its `[[inputs]]`; `cross_project`: `can-fd` is in another project).

## `phase` stays `learning`

The learning time runs on the frames' clock. With no frames arriving the cog advances from the wall clock, so it ends after **Learning time** regardless; if `frames_seen` is zero then, the baseline is empty and every id will look new. Check that `can-fd` is publishing.

## Too many `can.new_id` events

The bus has event-driven ids that were quiet while learning. Learn longer, or start the cog while the bus is busy.

## `skipped_records` grows

The cog fell behind the stream (a very busy bus). Frames are skipped, rates are understated and `rate_low` may fire; reduce the other load on the node.

## No events at all although the bus changed

The change is in data values, not ids, lengths or rates. This cog does not read data bytes.
