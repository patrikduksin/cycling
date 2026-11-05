import { describe, expect, test } from 'bun:test'
import * as Cause from 'effect/Cause'
import * as Effect from 'effect/Effect'
import * as Fiber from 'effect/Fiber'
import * as Queue from 'effect/Queue'
import * as Schema from 'effect/Schema'
import * as Stream from 'effect/Stream'
import { TestClock } from 'effect/testing'

import {
  Lease,
  type RelayEvent,
  RequestId,
  type ToBrowser,
} from '@cycling/protocol'

import { MONITOR_WINDOW_MS, deviceToRelay, relayToDevice } from './dataplane.ts'
import { SerialLinkLost } from './links.ts'
import type { Message } from './message.ts'
import { WRITE_QUEUE_FRAMES } from './policy.ts'

const run = <A, E>(program: Effect.Effect<A, E>): Promise<A> =>
  Effect.runPromise(program.pipe(Effect.provide(TestClock.layer())))

const settle = Effect.replicateEffect(Effect.yieldNow, 20, { discard: true })

const bytesOf = (message: Message): ReadonlyArray<number> =>
  message._tag === 'Traffic' ? Array.from(message.bytes) : []

describe('deviceToRelay', () => {
  test('sends coalesced frames, batches the monitor per 100 ms, then reports the lost port', () =>
    run(
      Effect.gen(function* () {
        const sent: Array<ReadonlyArray<number>> = []
        const queue = yield* Queue.unbounded<
          Uint8Array,
          SerialLinkLost | Cause.Done
        >()
        const fiber = yield* deviceToRelay({
          input: Stream.fromQueue(queue),
          send: bytes =>
            Effect.sync(() => {
              sent.push(Array.from(bytes))
              return true
            }),
          attempt: 3,
        }).pipe(Stream.runCollect, Effect.forkChild)

        yield* Queue.offer(queue, Uint8Array.of(1, 2, 3))
        yield* settle
        yield* TestClock.adjust('10 millis')
        yield* settle
        yield* Queue.offer(queue, Uint8Array.of(4))
        yield* settle
        yield* TestClock.adjust('100 millis')
        yield* settle
        yield* Queue.fail(queue, new SerialLinkLost())
        const messages = yield* Fiber.join(fiber)

        expect(sent).toEqual([[1, 2, 3], [4]])
        expect(messages.map(message => message._tag)).toEqual([
          'Traffic',
          'LostPort',
        ])
        expect(messages.flatMap(bytesOf)).toEqual([1, 2, 3, 4])
        expect(messages.at(-1)).toEqual({ _tag: 'LostPort', attempt: 3 })
      }),
    ))

  test('keeps draining the port while the relay is down and counts the drops', () =>
    run(
      Effect.gen(function* () {
        const queue = yield* Queue.unbounded<
          Uint8Array,
          SerialLinkLost | Cause.Done
        >()
        const fiber = yield* deviceToRelay({
          input: Stream.fromQueue(queue),
          send: () => Effect.succeed(false),
          attempt: 1,
        }).pipe(Stream.runCollect, Effect.forkChild)

        yield* Queue.offer(queue, Uint8Array.of(9, 9, 9))
        yield* settle
        yield* TestClock.adjust('10 millis')
        yield* settle
        yield* Queue.end(queue)
        const messages = yield* Fiber.join(fiber)

        const traffic = messages.find(message => message._tag === 'Traffic')
        expect(traffic).toMatchObject({
          direction: 'fromDevice',
          dropped: 3,
        })
      }),
    ))
})

describe('relayToDevice', () => {
  const id = Schema.decodeSync(RequestId)(7)
  const lease = Lease.make('0123456789abcdef')

  test('writes bytes to the port, passes control through and ends on close', () =>
    run(
      Effect.gen(function* () {
        const written: Array<ReadonlyArray<number>> = []
        const queue = yield* Queue.unbounded<
          RelayEvent<ToBrowser>,
          Cause.Done
        >()
        const fiber = yield* relayToDevice({
          events: Stream.fromQueue(queue),
          write: bytes =>
            Effect.sync(() => {
              written.push(Array.from(bytes))
              return true
            }),
          deliver: () => Effect.void,
          attempt: 2,
        }).pipe(Stream.runCollect, Effect.forkChild)

        yield* Queue.offer(queue, {
          _tag: 'Console',
          bytes: Uint8Array.of(1, 2),
        })
        yield* settle
        yield* Queue.offer(queue, {
          _tag: 'Control',
          message: { _tag: 'Op', id, op: { _tag: 'Release', lease } },
        })
        yield* Queue.offer(queue, { _tag: 'Pong' })
        yield* Queue.offer(queue, { _tag: 'Console', bytes: Uint8Array.of(3) })
        yield* settle
        yield* Queue.offer(queue, { _tag: 'Closed', code: 4410 })
        yield* Queue.end(queue)
        const messages = yield* Fiber.join(fiber)

        expect(written).toEqual([[1, 2], [3]])
        expect(messages.map(message => message._tag)).toEqual([
          'ReceivedFromRelay',
          'ClosedRelay',
          'Traffic',
        ])
        expect(messages.flatMap(bytesOf)).toEqual([1, 2, 3])
        expect(messages[1]).toEqual({
          _tag: 'ClosedRelay',
          attempt: 2,
          code: 4410,
        })
      }),
    ))

  test('write payloads go to the inbox in order and never reach update', () =>
    run(
      Effect.gen(function* () {
        const delivered: Array<number> = []
        const queue = yield* Queue.unbounded<
          RelayEvent<ToBrowser>,
          Cause.Done
        >()
        const fiber = yield* relayToDevice({
          events: Stream.fromQueue(queue),
          write: () => Effect.succeed(false),
          deliver: frame =>
            Effect.sync(() => {
              delivered.push(frame.offset)
            }),
          attempt: 1,
        }).pipe(Stream.runCollect, Effect.forkChild)

        for (const offset of [0, 15_360, 30_720]) {
          yield* Queue.offer(queue, {
            _tag: 'OpData',
            id,
            offset,
            bytes: new Uint8Array(4),
          })
        }
        yield* Queue.end(queue)
        const messages = yield* Fiber.join(fiber)

        expect(delivered).toEqual([0, 15_360, 30_720])
        expect(messages).toEqual([])
      }),
    ))

  test('control still flows while the port drops bytes', () =>
    run(
      Effect.gen(function* () {
        const queue = yield* Queue.unbounded<
          RelayEvent<ToBrowser>,
          Cause.Done
        >()
        const fiber = yield* relayToDevice({
          events: Stream.fromQueue(queue),
          write: () => Effect.succeed(false),
          deliver: () => Effect.void,
          attempt: 1,
        }).pipe(Stream.runCollect, Effect.forkChild)

        yield* Queue.offer(queue, {
          _tag: 'Console',
          bytes: Uint8Array.of(5, 5),
        })
        yield* Queue.offer(queue, {
          _tag: 'Control',
          message: { _tag: 'Op', id, op: { _tag: 'Enter', lease } },
        })
        yield* Queue.end(queue)
        const messages = yield* Fiber.join(fiber)

        expect(messages.map(message => message._tag)).toEqual([
          'ReceivedFromRelay',
          'Traffic',
        ])
        expect(messages[1]).toMatchObject({ direction: 'toDevice', dropped: 2 })
      }),
    ))

  test('a stuck port write never holds up control or a close, and the byte queue stays bounded', () =>
    run(
      Effect.gen(function* () {
        const written: Array<ReadonlyArray<number>> = []
        let calls = 0
        const queue = yield* Queue.unbounded<
          RelayEvent<ToBrowser>,
          Cause.Done
        >()
        const fiber = yield* relayToDevice({
          events: Stream.fromQueue(queue),
          write: bytes =>
            Effect.suspend(() => {
              calls += 1
              return calls === 1
                ? Effect.never
                : Effect.sync(() => {
                    written.push(Array.from(bytes))
                    return true
                  })
            }),
          deliver: () => Effect.void,
          attempt: 4,
        }).pipe(Stream.runCollect, Effect.forkChild)

        const sent = WRITE_QUEUE_FRAMES + 5
        for (let frame = 0; frame < sent; frame++) {
          yield* Queue.offer(queue, {
            _tag: 'Console',
            bytes: Uint8Array.of(frame),
          })
          if (frame === 0) {
            yield* settle
          }
        }
        yield* settle
        yield* TestClock.adjust(MONITOR_WINDOW_MS)
        yield* settle
        yield* Queue.offer(queue, {
          _tag: 'Control',
          message: { _tag: 'Op', id, op: { _tag: 'Enter', lease } },
        })
        yield* Queue.offer(queue, {
          _tag: 'Console',
          bytes: Uint8Array.of(0xaa),
        })
        yield* settle
        yield* Queue.offer(queue, { _tag: 'Closed', code: 4000 })
        yield* Queue.end(queue)
        const messages = yield* Fiber.join(fiber)

        expect(messages.map(message => message._tag)).toEqual([
          'Traffic',
          'ReceivedFromRelay',
          'ClosedRelay',
          'Traffic',
        ])
        const inFlight = 1
        expect(messages[0]).toMatchObject({
          direction: 'toDevice',
          dropped: sent - inFlight - WRITE_QUEUE_FRAMES,
        })
        expect(messages[3]).toMatchObject({
          direction: 'toDevice',
          dropped: 1 + inFlight + WRITE_QUEUE_FRAMES,
        })
        expect(written).toEqual([])
      }),
    ))
})
