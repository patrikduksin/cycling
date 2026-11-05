import { describe, expect, test } from 'bun:test'
import * as Option from 'effect/Option'

import { detectMissing, parseRelayBase, relayEndpoint } from './policy.ts'
import { session } from './test/fixtures.ts'

describe('detectMissing', () => {
  const ready = {
    isSecureContext: true,
    hasWebSerial: true,
    hasWebUsb: true,
    relayConfigured: true,
  }

  test('a fully supported page is fine', () => {
    expect(detectMissing(ready)).toEqual(Option.none())
  })

  test('names the first thing a visitor or the build can fix', () => {
    expect(
      detectMissing({ ...ready, isSecureContext: false, hasWebSerial: false }),
    ).toEqual(Option.some('secureContext'))
    expect(
      detectMissing({ ...ready, hasWebSerial: false, relayConfigured: false }),
    ).toEqual(Option.some('webSerial'))
    expect(detectMissing({ ...ready, relayConfigured: false })).toEqual(
      Option.some('relayUrl'),
    )
  })

  test('WebUSB alone is not required', () => {
    expect(detectMissing({ ...ready, hasWebUsb: false })).toEqual(Option.none())
  })
})

describe('relay address', () => {
  test.each([
    ['wss://relay.cycling.computer', 'wss://relay.cycling.computer'],
    ['https://relay.cycling.computer/', 'wss://relay.cycling.computer'],
    ['http://localhost:1337', 'ws://localhost:1337'],
    ['  ws://127.0.0.1:8787  ', 'ws://127.0.0.1:8787'],
  ])('%s becomes %s', (raw, expected) => {
    expect(parseRelayBase(raw)).toEqual(Option.some(expected))
  })

  test.each([
    undefined,
    '',
    '   ',
    'relay.cycling.computer',
    'ftp://relay.test',
  ])('%p is not configured', raw => {
    expect(parseRelayBase(raw)).toEqual(Option.none())
  })

  test('the browser endpoint is the versioned path under the base', () => {
    expect(relayEndpoint('wss://relay.test', session.code)).toBe(
      'wss://relay.test/v3/browser/7KQM4XP2',
    )
  })
})
