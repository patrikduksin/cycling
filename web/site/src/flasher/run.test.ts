import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Fiber from 'effect/Fiber'
import * as Ref from 'effect/Ref'
import * as Stream from 'effect/Stream'
import { TestClock } from 'effect/testing'

import { md5Hex } from '@cycling/flasher'
import {
  type Binary,
  type BrowserMsg,
  Lease,
  Md5,
  OP_PROGRESS_MS,
  RequestId,
  opDataFrames,
} from '@cycling/protocol'

import { pageSession } from '../adapters/flasher.ts'
import { demoFlash } from '../dev/image.ts'
import { memorySession } from '../dev/memorySession.ts'
import type { Message } from '../message.ts'
import type { RunnableOp } from '../model.ts'
import { makeInbox } from './inbox.ts'
import { runOp } from './run.ts'

const run = <A, E>(program: Effect.Effect<A, E>): Promise<A> =>
  Effect.runPromise(program.pipe(Effect.provide(TestClock.layer())))

const settle = Effect.replicateEffect(Effect.yieldNow, 50, { discard: true })

const lease = Lease.make('0123456789abcdef')
const id = RequestId.make(9)
const chip = { mac: '020000000001', secureBoot: false, flashEncryption: false }

const setup = (options: {
  readonly bytesPerSecond?: number
  readonly digestMs?: number
}) =>
  Effect.gen(function* () {
    const flash = demoFlash()
    const memory = yield* memorySession({
      flash,
      chip,
      bytesPerSecond: options.bytesPerSecond ?? Infinity,
      digestMs: options.digestMs ?? 0,
      pieceBytes: 64 * 1024,
    })
    const session = yield* pageSession(memory.session).pipe(Effect.orDie)
    const inbox = yield* makeInbox
    const sent: Array<Binary | BrowserMsg> = []
    const start = (op: RunnableOp) =>
      runOp({
        attempt: 3,
        id,
        op,
        session,
        inbox,
        send: item =>
          Effect.sync(() => {
            sent.push(item)
            return true
          }),
      }).pipe(Stream.runCollect, Effect.forkChild)
    return { flash, memory, inbox, sent, start }
  })

const finishedOf = (messages: ReadonlyArray<Message>) => messages.at(-1)

const md5 = (bytes: Uint8Array) => Md5.make(md5Hex(bytes))

const nth = <A>(items: ReadonlyArray<A>, index: number): A => {
  const item = items[index]
  if (item === undefined) {
    throw new Error(`no item ${index}`)
  }
  return item
}

describe('runOp', () => {
  test('a read sends contiguous OpData frames of the region, then Read with the chip digest', () =>
    run(
      Effect.gen(function* () {
        const { flash, sent, start } = yield* setup({})
        const region = { address: 0x8000, size: 100 * 1024 }
        const messages = yield* Fiber.join(
          yield* start({ _tag: 'Read', lease, region }),
        )

        const frames = sent.filter(item => item._tag === 'OpData')
        let next = 0
        for (const frame of frames) {
          expect(frame.id).toBe(id)
          expect(frame.offset).toBe(next)
          next += frame.bytes.byteLength
        }
        expect(next).toBe(region.size)
        const received = new Uint8Array(region.size)
        for (const frame of frames) {
          received.set(frame.bytes, frame.offset)
        }
        const expected = flash.slice(0x8000, 0x8000 + region.size)
        expect(received).toEqual(expected)
        expect(finishedOf(messages)).toEqual({
          _tag: 'FinishedOp',
          attempt: 3,
          id,
          finished: { _tag: 'Read', md5: md5(expected) },
        })
      }),
    ))

  test('a write whose payload has a hole is transferGap and never reaches flash', () =>
    run(
      Effect.gen(function* () {
        const { memory, inbox, start } = yield* setup({})
        const payload = new Uint8Array(40 * 1024).fill(7)
        const frames = [...opDataFrames(id, payload)]
        yield* inbox.offer(nth(frames, 0))
        yield* inbox.offer(nth(frames, 2))
        const fiber = yield* start({
          _tag: 'Write',
          lease,
          region: { address: 0x760000, size: payload.byteLength },
          md5: md5(payload),
        })
        const messages = yield* Fiber.join(fiber)

        expect(finishedOf(messages)).toMatchObject({
          finished: { _tag: 'Failed', reason: 'transferGap' },
        })
        expect(yield* Ref.get(memory.writes)).toBe(0)
      }),
    ))

  test('a payload whose md5 differs from the announced one is payloadMismatch and never reaches flash', () =>
    run(
      Effect.gen(function* () {
        const { flash, memory, inbox, start } = yield* setup({})
        const payload = new Uint8Array(20 * 1024).fill(7)
        const fiber = yield* start({
          _tag: 'Write',
          lease,
          region: { address: 0x760000, size: payload.byteLength },
          md5: md5(new Uint8Array(20 * 1024).fill(8)),
        })
        yield* settle
        yield* Effect.forEach(opDataFrames(id, payload), inbox.offer, {
          discard: true,
        })
        const messages = yield* Fiber.join(fiber)

        expect(finishedOf(messages)).toMatchObject({
          finished: { _tag: 'Failed', reason: 'payloadMismatch' },
        })
        expect(yield* Ref.get(memory.writes)).toBe(0)
        expect(flash[0x760000]).toBe(0xff)
      }),
    ))

  test('a whole payload, sent before the op runs, is written and answered with the chip digest', () =>
    run(
      Effect.gen(function* () {
        const { flash, inbox, start } = yield* setup({})
        const payload = new Uint8Array(70 * 1024).map((_, i) => i % 251)
        yield* Effect.forEach(opDataFrames(id, payload), inbox.offer, {
          discard: true,
        })
        const messages = yield* Fiber.join(
          yield* start({
            _tag: 'Write',
            lease,
            region: { address: 0x760000, size: payload.byteLength },
            md5: md5(payload),
          }),
        )

        expect(finishedOf(messages)).toMatchObject({
          finished: { _tag: 'Written', md5: md5(payload) },
        })
        expect(flash.slice(0x760000, 0x760000 + payload.byteLength)).toEqual(
          payload,
        )
      }),
    ))

  test('a payload that stops arriving fails transferGap after the stall time', () =>
    run(
      Effect.gen(function* () {
        const { inbox, start } = yield* setup({})
        const payload = new Uint8Array(40 * 1024)
        yield* inbox.offer(nth([...opDataFrames(id, payload)], 0))
        const fiber = yield* start({
          _tag: 'Write',
          lease,
          region: { address: 0x760000, size: payload.byteLength },
          md5: md5(payload),
        })
        yield* settle
        yield* TestClock.adjust('21 seconds')
        const messages = yield* Fiber.join(fiber)

        expect(finishedOf(messages)).toMatchObject({
          finished: { _tag: 'Failed', reason: 'transferGap' },
        })
      }),
    ))

  test.each([
    ['a slow read', { bytesPerSecond: 16 * 1024, digestMs: 0 }],
    [
      'a digest the chip reports nothing during',
      { bytesPerSecond: Infinity, digestMs: 5000 },
    ],
  ])(
    '%s reports progress at least once a second, and nothing after it ends',
    (_, speed) =>
      run(
        Effect.gen(function* () {
          const { sent, start } = yield* setup(speed)
          const fiber = yield* start({
            _tag: speed.digestMs > 0 ? 'Digest' : 'Read',
            lease,
            region: { address: 0x10000, size: 80 * 1024 },
          })
          const progressAt: Array<number> = []
          for (let second = 0; second < 5; second++) {
            yield* settle
            progressAt.push(
              sent.filter(item => item._tag === 'OpProgress').length,
            )
            yield* TestClock.adjust(OP_PROGRESS_MS)
          }
          const messages = yield* Fiber.join(fiber)
          const progress = sent.filter(item => item._tag === 'OpProgress')

          for (let second = 1; second < progressAt.length; second++) {
            expect(progressAt[second] ?? 0).toBeGreaterThan(
              progressAt[second - 1] ?? 0,
            )
          }
          expect(progress[0]).toEqual({
            _tag: 'OpProgress',
            id,
            done: 0,
            total: 80 * 1024,
          })
          expect(messages.filter(m => m._tag === 'ProgressedOp').length).toBe(
            progress.length,
          )
          expect(messages.at(-1)?._tag).toBe('FinishedOp')
          yield* TestClock.adjust(OP_PROGRESS_MS * 3)
          expect(sent.filter(item => item._tag === 'OpProgress').length).toBe(
            progress.length,
          )
        }),
      ),
  )
})
