import { describe, expect, test } from 'bun:test'
import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Schedule from 'effect/Schedule'

import { BACKOFF } from '@cycling/protocol'

import { type Dropped, endingOf, redialSchedule } from '../src/RelayClient.ts'

describe('relay close codes', () => {
  test('a session end, a replacement or a refusal stops; anything else redials', () => {
    expect(endingOf(4000)).toEqual(
      Option.some({ _tag: 'Ended', reason: 'visitorDisconnected' }),
    )
    expect(endingOf(4001)).toEqual(
      Option.some({ _tag: 'Ended', reason: 'visitorGone' }),
    )
    expect(endingOf(4408)).toEqual(
      Option.some({ _tag: 'Ended', reason: 'expired' }),
    )
    expect(endingOf(4410)).toEqual(Option.some({ _tag: 'Replaced' }))
    const refusals = [4401, 4404, 4426, 1008, 4429, 4409].map(code =>
      Option.match(endingOf(code), {
        onNone: () => 'redial',
        onSome: ending =>
          ending._tag === 'Rejected' ? ending.error._tag : ending._tag,
      }),
    )
    expect(refusals).toEqual([
      'Unauthorized',
      'SessionNotFound',
      'UnsupportedVersion',
      'UnsupportedVersion',
      'RelayUnreachable',
      'SessionNotFound',
    ])
    for (const code of [1000, 1001, 1006, 1011, 4999]) {
      expect(endingOf(code)).toEqual(Option.none())
    }
  })
})

const delays = (drops: ReadonlyArray<Dropped>) =>
  Effect.runPromise(
    Effect.gen(function* () {
      const step = yield* Schedule.toStep(redialSchedule)
      const out: Array<number> = []
      for (const dropped of drops) {
        const [, delay] = yield* step(0, dropped)
        out.push(Duration.toMillis(delay))
      }
      return out
    }),
  )

const failed: Dropped = { _tag: 'Dropped', welcomed: false }
const welcomed: Dropped = { _tag: 'Dropped', welcomed: true }

describe('redial backoff', () => {
  test('doubles from BACKOFF.baseMs and stays at BACKOFF.maxMs', async () => {
    expect(await delays([welcomed, ...Array(7).fill(failed)])).toEqual([
      500, 1000, 2000, 4000, 8000, 10_000, 10_000, 10_000,
    ])
    expect(BACKOFF).toEqual({ baseMs: 500, maxMs: 10_000 })
  })

  test('a drop after a Welcome starts over', async () => {
    expect(await delays([welcomed, failed, failed, welcomed, failed])).toEqual([
      500, 1000, 2000, 500, 1000,
    ])
  })
})
