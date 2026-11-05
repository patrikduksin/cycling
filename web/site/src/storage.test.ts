import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'

import {
  clearStoredSession,
  readStoredSession,
  writeStoredSession,
} from './storage.ts'
import { baud, epoch, identity, session } from './test/fixtures.ts'

const entries = new Map<string, string>()
const original = Object.getOwnPropertyDescriptor(globalThis, 'sessionStorage')

beforeEach(() => {
  entries.clear()
  Object.defineProperty(globalThis, 'sessionStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => entries.get(key) ?? null,
      setItem: (key: string, value: string) => entries.set(key, value),
      removeItem: (key: string) => entries.delete(key),
    },
  })
})

afterEach(() => {
  if (original === undefined) {
    Reflect.deleteProperty(globalThis, 'sessionStorage')
  } else {
    Object.defineProperty(globalThis, 'sessionStorage', original)
  }
})

const stored = { session, identity, baud: baud(921_600), epoch: epoch(3) }

describe('stored session', () => {
  test('survives a write and a read', async () => {
    await Effect.runPromise(writeStoredSession(stored))
    expect(await Effect.runPromise(readStoredSession)).toEqual(
      Option.some(stored),
    )
  })

  test('holds the code, owner key and ids, and no serial number field', async () => {
    await Effect.runPromise(writeStoredSession(stored))
    const raw = [...entries.values()].join('')
    expect(JSON.parse(raw)).toEqual({
      session: { code: '7KQM4XP2', ownerKey: session.ownerKey },
      identity: { vendorId: 0x303a, productId: 0x1001 },
      baud: 921_600,
      epoch: 3,
    })
  })

  test('is gone after a clear', async () => {
    await Effect.runPromise(writeStoredSession(stored))
    await Effect.runPromise(clearStoredSession)
    expect(await Effect.runPromise(readStoredSession)).toEqual(Option.none())
  })

  test('garbage in storage reads as no session', async () => {
    entries.set('cycling.remote.session', '{"session":{"code":"nope"}}')
    expect(await Effect.runPromise(readStoredSession)).toEqual(Option.none())
  })

  test('storage that throws reads as no session and a write is skipped', async () => {
    Object.defineProperty(globalThis, 'sessionStorage', {
      configurable: true,
      value: {
        getItem: () => {
          throw new DOMException('blocked', 'SecurityError')
        },
        setItem: () => {
          throw new DOMException('blocked', 'SecurityError')
        },
        removeItem: () => {
          throw new DOMException('blocked', 'SecurityError')
        },
      },
    })
    expect(await Effect.runPromise(readStoredSession)).toEqual(Option.none())
    await Effect.runPromise(writeStoredSession(stored))
    await Effect.runPromise(clearStoredSession)
  })
})
