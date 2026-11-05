import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Pull from 'effect/Pull'
import * as Schedule from 'effect/Schedule'
import * as Sink from 'effect/Sink'
import * as Stream from 'effect/Stream'

import { COALESCE } from './wire.ts'

/** At most `maxBytes` of bytes, then at most one non-byte item that closed it. */
interface Batch<C> {
  readonly parts: ReadonlyArray<Uint8Array>
  readonly size: number
  readonly after: ReadonlyArray<C>
}

const isBytes = <C>(item: Uint8Array | C): item is Uint8Array =>
  item instanceof Uint8Array

/**
 * Fills one frame. A chunk that does not fit is split: the part that fits
 * goes in, the rest is a leftover that starts the next frame at once.
 */
const frameSink = <C>(
  maxBytes: number,
): Sink.Sink<Batch<C>, Uint8Array | C, Uint8Array | C> =>
  Sink.fromTransform(upstream => {
    const parts: Array<Uint8Array> = []
    let size = 0
    const batch = (after: ReadonlyArray<C> = []): Batch<C> => ({
      parts,
      size,
      after,
    })
    return Effect.gen(function* () {
      while (true) {
        const items = yield* upstream
        for (let index = 0; index < items.length; index++) {
          const item = items[index]
          if (item === undefined) continue
          const rest = items.slice(index + 1)
          if (!isBytes(item)) {
            return [batch([item]), toLeftover(rest)] as const
          }
          const room = maxBytes - size
          if (item.length < room) {
            parts.push(item)
            size += item.length
            continue
          }
          parts.push(item.subarray(0, room))
          size += room
          const tail = item.subarray(room)
          return [
            batch(),
            toLeftover(tail.length > 0 ? [tail, ...rest] : rest),
          ] as const
        }
      }
    }).pipe(Pull.catchDone(() => Effect.succeed([batch()] as const)))
  })

const toLeftover = <A>(
  items: ReadonlyArray<A>,
): readonly [A, ...Array<A>] | undefined => {
  const [first, ...others] = items
  return first === undefined ? undefined : [first, ...others]
}

const join = (batch: Batch<unknown>): Uint8Array => {
  const out = new Uint8Array(batch.size)
  let offset = 0
  for (const part of batch.parts) {
    out.set(part, offset)
    offset += part.length
  }
  return out
}

/**
 * Relay cost is per message, so byte chunks are merged into frames of at most
 * `maxBytes`, flushed when the budget is full or `maxWaitMs` after the first
 * pending byte, whichever comes first. A non-byte item (a control message)
 * flushes what is pending and passes through in order, so it never overtakes
 * the bytes written before it.
 */
export const coalesceBytes =
  (
    options: {
      readonly maxBytes: number
      readonly maxWaitMs: number
    } = COALESCE,
  ) =>
  <C, E, R>(
    self: Stream.Stream<Uint8Array | C, E, R>,
  ): Stream.Stream<Uint8Array | C, E, R> =>
    self.pipe(
      Stream.aggregateWithin(
        frameSink<C>(options.maxBytes),
        Schedule.spaced(Duration.millis(options.maxWaitMs)),
      ),
      Stream.flatMap(batch =>
        Stream.fromIterable<Uint8Array | C>(
          batch.size > 0 ? [join(batch), ...batch.after] : batch.after,
        ),
      ),
    )
