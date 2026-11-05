import * as Array from 'effect/Array'
import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as FiberHandle from 'effect/FiberHandle'
import * as Match from 'effect/Match'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import {
  type OpData,
  type RelayEvent,
  type ToBrowser,
  coalesceBytes,
} from '@cycling/protocol'

import type { SerialLinkLost } from './links.ts'
import { Message } from './message.ts'
import type { Direction } from './model.ts'
import { WRITE_QUEUE_FRAMES } from './policy.ts'

/** The monitor hears about traffic once per window, not once per chunk. */
export const MONITOR_WINDOW_MS = 100

interface Pending {
  readonly chunks: ReadonlyArray<Uint8Array>
  readonly dropped: number
}

const concat = (chunks: ReadonlyArray<Uint8Array>): Uint8Array => {
  const all = new Uint8Array(
    Array.reduce(chunks, 0, (n, chunk) => n + chunk.length),
  )
  Array.reduce(chunks, 0, (offset, chunk) => {
    all.set(chunk, offset)
    return offset + chunk.length
  })
  return all
}

interface Meter {
  readonly record: (
    bytes: Uint8Array,
    delivered: boolean,
  ) => Effect.Effect<void>
  readonly drain: Effect.Effect<ReadonlyArray<Message>>
}

const makeMeter = (direction: Direction): Effect.Effect<Meter> =>
  Ref.make<Pending>({ chunks: [], dropped: 0 }).pipe(
    Effect.map(pending => ({
      record: (bytes, delivered) =>
        Ref.update(pending, state => ({
          chunks: [...state.chunks, bytes],
          dropped: state.dropped + (delivered ? 0 : bytes.length),
        })),
      drain: Ref.getAndSet(pending, { chunks: [], dropped: 0 }).pipe(
        Effect.map(({ chunks, dropped }) =>
          chunks.length === 0
            ? []
            : [Message.Traffic({ direction, bytes: concat(chunks), dropped })],
        ),
      ),
    })),
  )

/**
 * Runs `body`, then emits `tail`. Whatever the body recorded in the meter
 * reaches `update` as one Traffic Message per window, and once more when the
 * body ends, ahead of `tail`.
 */
const withTraffic = <R>(
  direction: Direction,
  body: (meter: Meter) => Stream.Stream<Message, never, R>,
  tail: Stream.Stream<Message>,
): Stream.Stream<Message, never, R> =>
  Stream.unwrap(
    makeMeter(direction).pipe(
      Effect.map(meter =>
        Stream.merge(
          body(meter).pipe(
            Stream.concat(
              Stream.fromEffect(meter.drain).pipe(Stream.flattenIterable),
            ),
            Stream.concat(tail),
          ),
          Stream.tick(Duration.millis(MONITOR_WINDOW_MS)).pipe(
            Stream.drop(1),
            Stream.mapEffect(() => meter.drain),
            Stream.flattenIterable,
          ),
          { haltStrategy: 'left' },
        ),
      ),
    ),
  )

/**
 * Device bytes toward the relay, coalesced into frames. The port keeps being
 * read while the relay is down, so Chrome's small receive buffer never
 * overruns; those bytes count as dropped. A lost port ends the stream with
 * one `LostPort` for this attempt.
 */
export const deviceToRelay = <R>(options: {
  readonly input: Stream.Stream<Uint8Array, SerialLinkLost>
  readonly send: (bytes: Uint8Array) => Effect.Effect<boolean, never, R>
  readonly attempt: number
}): Stream.Stream<Message, never, R> =>
  withTraffic(
    'fromDevice',
    meter =>
      options.input.pipe(
        coalesceBytes(),
        Stream.mapEffect(bytes =>
          options
            .send(bytes)
            .pipe(Effect.flatMap(delivered => meter.record(bytes, delivered))),
        ),
        Stream.drain,
        Stream.catch(() => Stream.empty),
      ),
    Stream.make(Message.LostPort({ attempt: options.attempt })),
  )

interface WritePath<R> {
  /** Never waits: a frame the full queue cannot take is dropped. */
  readonly offer: (bytes: Uint8Array) => Effect.Effect<void>
  /** Drops what is queued and the write in flight, so nothing stale reaches a reset device. */
  readonly abandon: Effect.Effect<void, never, R>
}

/**
 * Relay bytes wait in a bounded queue and one fiber writes them to the port.
 * A port that stops taking bytes stalls that fiber only; control frames are
 * read past it at once.
 */
const makeWritePath = <R>(
  write: (bytes: Uint8Array) => Effect.Effect<boolean, never, R>,
  meter: Meter,
): Effect.Effect<WritePath<R>, never, R | Scope.Scope> =>
  Effect.gen(function* () {
    const queue = yield* Queue.dropping<Uint8Array>(WRITE_QUEUE_FRAMES)
    const writer = yield* FiberHandle.make<never>()
    const drain = Effect.forever(
      Queue.take(queue).pipe(
        Effect.flatMap(bytes =>
          write(bytes).pipe(
            Effect.onInterrupt(() => meter.record(bytes, false)),
            Effect.flatMap(delivered => meter.record(bytes, delivered)),
          ),
        ),
      ),
    )
    const start = FiberHandle.run(writer, drain)
    yield* start
    return {
      offer: bytes =>
        Queue.offer(queue, bytes).pipe(
          Effect.flatMap(queued =>
            queued ? Effect.void : meter.record(bytes, false),
          ),
        ),
      abandon: FiberHandle.clear(writer).pipe(
        Effect.andThen(Queue.clear(queue)),
        Effect.flatMap(left =>
          Effect.forEach(left, bytes => meter.record(bytes, false), {
            discard: true,
          }),
        ),
        Effect.andThen(start),
        Effect.asVoid,
      ),
    }
  })

/**
 * Relay frames in arrival order: console bytes go to the port's write queue
 * (counted as dropped when it is down, full or held by the flasher), write
 * payloads go to the flasher's inbox, control becomes a Message, and a close
 * ends the stream with its code, abandoning the bytes still waiting for the
 * port. Control keeps flowing while the port is gone or stuck.
 */
export const relayToDevice = <R>(options: {
  readonly events: Stream.Stream<RelayEvent<ToBrowser>>
  readonly write: (bytes: Uint8Array) => Effect.Effect<boolean, never, R>
  readonly deliver: (frame: OpData) => Effect.Effect<void, never, R>
  readonly attempt: number
}): Stream.Stream<Message, never, R> =>
  withTraffic(
    'toDevice',
    meter =>
      Stream.unwrap(
        makeWritePath(options.write, meter).pipe(
          Effect.map(path => {
            const handle = (
              event: RelayEvent<ToBrowser>,
            ): Effect.Effect<ReadonlyArray<Message>, never, R> =>
              Match.value(event).pipe(
                Match.withReturnType<
                  Effect.Effect<ReadonlyArray<Message>, never, R>
                >(),
                Match.tagsExhaustive({
                  Console: ({ bytes }) => path.offer(bytes).pipe(Effect.as([])),
                  OpData: frame => options.deliver(frame).pipe(Effect.as([])),
                  Control: ({ message }) =>
                    Effect.succeed([Message.ReceivedFromRelay({ message })]),
                  Pong: () => Effect.succeed([]),
                  Closed: ({ code }) =>
                    path.abandon.pipe(
                      Effect.as([
                        Message.ClosedRelay({ attempt: options.attempt, code }),
                      ]),
                    ),
                }),
              )
            return options.events.pipe(
              Stream.mapEffect(handle),
              Stream.flattenIterable,
            )
          }),
        ),
      ),
    Stream.empty,
  )
