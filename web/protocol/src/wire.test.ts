import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import * as Stream from 'effect/Stream'

import {
  backoffMs,
  BACKOFF,
  CloseCode,
  type CloseMeaning,
  closeMeaning,
  closeName,
  Code,
  coalesceBytes,
  COALESCE,
  type EndReason,
  formatBytes,
  parseSessionPath,
  sessionPath,
} from './index.ts'

const code = Schema.decodeSync(Code)('7KQM4XP2')

const MEANINGS: ReadonlyArray<readonly [number, CloseMeaning]> = [
  [
    CloseCode.visitorDisconnected,
    { _tag: 'Ended', reason: 'visitorDisconnected' },
  ],
  [CloseCode.visitorGone, { _tag: 'Ended', reason: 'visitorGone' }],
  [CloseCode.expired, { _tag: 'Ended', reason: 'expired' }],
  [CloseCode.replaced, { _tag: 'Replaced' }],
  [CloseCode.codeInUse, { _tag: 'Remint' }],
  [CloseCode.unauthorized, { _tag: 'Refused', why: 'unauthorized' }],
  [CloseCode.notFound, { _tag: 'Refused', why: 'notFound' }],
  [
    CloseCode.unsupportedVersion,
    { _tag: 'Refused', why: 'unsupportedVersion' },
  ],
  [CloseCode.rateLimited, { _tag: 'Refused', why: 'rateLimited' }],
  [CloseCode.protocolViolation, { _tag: 'Refused', why: 'protocolViolation' }],
  [1000, { _tag: 'Transient' }],
  [1006, { _tag: 'Transient' }],
  [1011, { _tag: 'Transient' }],
]

describe('close codes', () => {
  test.each(MEANINGS)('%d means %o', (closeCode, meaning) => {
    expect(closeMeaning(closeCode)).toEqual(meaning)
  })

  test('every end reason closes with the code of the same name', () => {
    const reasons: ReadonlyArray<EndReason> = [
      'visitorDisconnected',
      'visitorGone',
      'expired',
    ]
    for (const reason of reasons) {
      expect(closeMeaning(CloseCode[reason])).toEqual({ _tag: 'Ended', reason })
      expect(closeName(CloseCode[reason])).toEqual(Option.some(reason))
    }
    expect(closeName(1006)).toEqual(Option.none())
  })
})

describe('session paths', () => {
  test('a built path parses back to its role, version and code', () => {
    expect(parseSessionPath(sessionPath('agent', code))).toEqual(
      Option.some({ role: 'agent', version: 1, code: Option.some(code) }),
    )
    expect(parseSessionPath('/v2/browser/7KQM4XP2')).toEqual(
      Option.some({ role: 'browser', version: 2, code: Option.some(code) }),
    )
  })

  test('a malformed code is still a session path; anything else is not', () => {
    expect(parseSessionPath('/v1/agent/nope')).toEqual(
      Option.some({ role: 'agent', version: 1, code: Option.none() }),
    )
    expect(parseSessionPath('/v1/admin/7KQM4XP2')).toEqual(Option.none())
    expect(parseSessionPath('/v1/agent/7KQM4XP2/x')).toEqual(Option.none())
    expect(parseSessionPath('/')).toEqual(Option.none())
  })
})

describe('backoff', () => {
  test('doubles from the base and stops at the cap', () => {
    expect([1, 2, 3, 4, 5, 6, 7].map(backoffMs)).toEqual([
      500, 1000, 2000, 4000, 8000, 10_000, 10_000,
    ])
    expect(backoffMs(0)).toBe(BACKOFF.baseMs)
  })
})

describe('formatBytes', () => {
  test.each([
    [0, '0 B'],
    [1023, '1023 B'],
    [1024, '1.0 KiB'],
    [4300, '4.2 KiB'],
    [20_000, '20 KiB'],
    [13 * 1024 * 1024, '13 MiB'],
  ])('%d is %s', (count, text) => {
    expect(formatBytes(count)).toBe(text)
  })
})

const collect = <A>(stream: Stream.Stream<A>) =>
  Effect.runPromise(Stream.runCollect(stream))

const sizes = (items: ReadonlyArray<Uint8Array | string>) =>
  items.map(item => (typeof item === 'string' ? item : item.length))

describe('coalesceBytes', () => {
  test('a burst of small chunks becomes full frames, bytes and order intact', async () => {
    const chunks = Array.from({ length: 1000 }, (_, i) =>
      new Uint8Array(9).fill(i % 256),
    )
    const frames = await collect(
      Stream.fromIterable(chunks).pipe(coalesceBytes()),
    )
    expect(frames.every(frame => frame.length <= COALESCE.maxBytes)).toBe(true)
    expect(frames.length).toBe(Math.ceil(9000 / COALESCE.maxBytes))
    expect(Buffer.concat(frames)).toEqual(Buffer.concat(chunks))
  })

  test('one chunk bigger than the budget is split at the budget', async () => {
    const big = new Uint8Array(COALESCE.maxBytes * 2 + 5).map((_, i) => i)
    const frames = await collect(Stream.make(big).pipe(coalesceBytes()))
    expect(sizes(frames)).toEqual([COALESCE.maxBytes, COALESCE.maxBytes, 5])
  })

  test('a control item ends the frame before it and keeps its place', async () => {
    const items = await collect(
      Stream.make<ReadonlyArray<Uint8Array | string>>(
        new Uint8Array(3),
        new Uint8Array(4),
        'reset',
        new Uint8Array(5),
      ).pipe(coalesceBytes()),
    )
    expect(sizes(items)).toEqual([7, 'reset', 5])
  })

  test('bytes further apart than the window are not held for each other', async () => {
    const spaced = Stream.make(new Uint8Array(1), new Uint8Array(2)).pipe(
      Stream.tap(() => Effect.sleep('40 millis')),
    )
    const frames = await collect(
      spaced.pipe(coalesceBytes({ maxBytes: 4096, maxWaitMs: 5 })),
    )
    expect(sizes(frames)).toEqual([1, 2])
  })
})
