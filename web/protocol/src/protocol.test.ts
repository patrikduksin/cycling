import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import {
  type AgentMsg,
  agentProtocols,
  type BrowserMsg,
  browserProtocols,
  Code,
  decodeFromAgent,
  decodeFromBrowser,
  decodeToAgent,
  decodeToBrowser,
  encodeAgentMsg,
  encodeBrowserMsg,
  encodeRelayMsg,
  Epoch,
  formatCode,
  type Frame,
  MAX_FRAME_BYTES,
  mintCode,
  mintOwnerKey,
  parseCode,
  parseOffered,
  type ProtocolViolation,
  recognitionOf,
  RequestId,
  RESET_PLANS,
  type RelayMsg,
} from './index.ts'

const run = <A, E>(effect: Effect.Effect<A, E>) => Effect.runSyncExit(effect)
const key = mintOwnerKey(new Uint8Array(16).fill(7))
const id = Schema.decodeSync(RequestId)(3)
const epoch = Schema.decodeSync(Epoch)(1)

const browserMessages: ReadonlyArray<BrowserMsg> = [
  {
    _tag: 'DeviceAnnounced',
    device: {
      _tag: 'serial',
      identity: { vendorId: 0x303a, productId: 0x1001 },
    },
  },
  {
    _tag: 'DeviceAnnounced',
    device: {
      _tag: 'usb',
      descriptor: {
        vendorId: 0x22d9,
        productId: 0x2765,
        usbVersion: '2.0',
        deviceClass: 6,
        deviceSubclass: 1,
        deviceProtocol: 1,
        deviceVersion: '4.4.0',
        productName: 'Camera',
      },
    },
  },
  {
    _tag: 'PortChanged',
    status: { epoch, state: { _tag: 'Open', baud: 115200 } },
  },
  {
    _tag: 'PortChanged',
    status: { epoch, state: { _tag: 'Failed', reason: 'reacquireTimeout' } },
  },
  { _tag: 'ResetResult', id, outcome: 'done' },
  { _tag: 'LineCodingResult', id, outcome: 'portNotOpen' },
  { _tag: 'Bye' },
]
const agentMessages: ReadonlyArray<AgentMsg> = [
  { _tag: 'Reset', id, kind: 'usbJtagBootloader' },
  { _tag: 'Reset', id, kind: 'hardReset' },
  { _tag: 'SetLineCoding', id, baud: 921600 },
]
const relayMessages: ReadonlyArray<RelayMsg> = [
  {
    _tag: 'Welcome',
    role: 'agent',
    snapshot: {
      device: null,
      port: null,
      expiresAt: 1,
      browserPresent: true,
      agentPresent: false,
    },
  },
  { _tag: 'PeerPresence', role: 'browser', present: false },
]

const control = <C>(message: C): Exit.Exit<Frame<C>, ProtocolViolation> =>
  Exit.succeed({ _tag: 'Control', message })

describe('frames', () => {
  test('every message round-trips through its sender and receiver decoders', () => {
    for (const message of browserMessages) {
      const text = encodeBrowserMsg(message)
      expect(run(decodeFromBrowser(text))).toEqual(control(message))
      expect(run(decodeToAgent(text))).toEqual(control(message))
    }
    for (const message of agentMessages) {
      const text = encodeAgentMsg(message)
      expect(run(decodeFromAgent(text))).toEqual(control(message))
      expect(run(decodeToBrowser(text))).toEqual(control(message))
    }
    for (const message of relayMessages) {
      const text = encodeRelayMsg(message)
      expect(run(decodeToAgent(text))).toEqual(control(message))
      expect(run(decodeToBrowser(text))).toEqual(control(message))
    }
  })

  test('a wrong-direction message fails at the boundary', () => {
    const reset = encodeAgentMsg({ _tag: 'Reset', id, kind: 'hardReset' })
    expect(Exit.isFailure(run(decodeFromBrowser(reset)))).toBe(true)
    expect(Exit.isFailure(run(decodeToAgent(reset)))).toBe(true)
    const presence = encodeRelayMsg({
      _tag: 'PeerPresence',
      role: 'agent',
      present: true,
    })
    expect(Exit.isFailure(run(decodeFromAgent(presence)))).toBe(true)
  })

  test('a serial number anywhere in a device is refused, not stripped', () => {
    const smuggled = JSON.stringify({
      _tag: 'DeviceAnnounced',
      device: {
        _tag: 'serial',
        identity: { vendorId: 1, productId: 2, serialNumber: 'AA:BB' },
      },
    })
    expect(Exit.isFailure(run(decodeFromBrowser(smuggled)))).toBe(true)
    const usb = JSON.stringify({
      _tag: 'DeviceAnnounced',
      device: {
        _tag: 'usb',
        descriptor: {
          vendorId: 1,
          productId: 2,
          usbVersion: '2.0',
          deviceClass: 0,
          deviceSubclass: 0,
          deviceProtocol: 0,
          deviceVersion: '1.0',
          serialNumber: 'AA:BB',
        },
      },
    })
    expect(Exit.isFailure(run(decodeFromBrowser(usb)))).toBe(true)
  })

  test('binary is bytes untouched, capped at MAX_FRAME_BYTES', () => {
    const bytes = new Uint8Array([0, 0xff, 0x11, 0x13, 0xc0])
    expect(run(decodeToAgent(bytes.buffer))).toEqual(
      Exit.succeed({ _tag: 'Bytes', bytes }),
    )
    const big = new Uint8Array(MAX_FRAME_BYTES + 1)
    expect(Exit.isFailure(run(decodeFromBrowser(big)))).toBe(true)
  })

  test('pong is keepalive, other text that is not JSON is malformed', () => {
    expect(run(decodeToAgent('pong'))).toEqual(Exit.succeed({ _tag: 'Pong' }))
    expect(Exit.isFailure(run(decodeToAgent('{nope')))).toBe(true)
  })
})

describe('codes and keys', () => {
  test('a minted code is 8 Crockford characters and reads back as typed', () => {
    const code = mintCode(new Uint8Array([0xff, 0xff, 0xff, 0xff, 0xff]))
    expect(code).toBe(Schema.decodeSync(Code)('ZZZZZZZZ'))
    expect(mintCode(new Uint8Array(5))).toBe(
      Schema.decodeSync(Code)('00000000'),
    )
    expect(formatCode(code)).toBe('ZZZZ-ZZZZ')
    expect(parseCode('zzzz-zzzz')).toEqual(Option.some(code))
    expect(parseCode('o0il-1111')).toEqual(
      Option.some(Schema.decodeSync(Code)('00111111')),
    )
    expect(parseCode('ZZZZ')).toEqual(Option.none())
  })

  test('owner keys are 22 base64url characters', () => {
    expect(key).toMatch(/^[A-Za-z0-9_-]{22}$/)
    expect(mintOwnerKey(new Uint8Array(16).fill(0xfb))).toMatch(/^[-_]/)
  })

  test('offered subprotocols parse back to their credentials', () => {
    const agent = parseOffered(agentProtocols('abc123').join(', '))
    expect(agent).toEqual({
      entries: ['cycling.v1', 'auth.abc123'],
      version: true,
      token: Option.some('abc123'),
      ownerKey: Option.none(),
      selected: Option.some('cycling.v1'),
    })
    const browser = parseOffered(browserProtocols(key).join(','))
    expect(browser.ownerKey).toEqual(Option.some(key))
    expect(parseOffered('cycling.v2').version).toBe(false)
    expect(parseOffered(null).version).toBe(false)
    expect(parseOffered('cycling.v1, own.short').ownerKey).toEqual(
      Option.none(),
    )
  })

  test('the echo is the version token, else the first non-credential, never a credential', () => {
    const echo = (header: string | null) => parseOffered(header).selected
    expect(echo('auth.secret, cycling.v1')).toEqual(Option.some('cycling.v1'))
    expect(echo('auth.secret, cycling.v0')).toEqual(Option.some('cycling.v0'))
    expect(echo('auth.secret, own.key')).toEqual(Option.none())
    expect(echo(null)).toEqual(Option.none())
  })
})

describe('reset plans', () => {
  test('the bootloader plan is a level sequence the recognizer can match', () => {
    expect(recognitionOf('usbJtagBootloader')).toEqual({
      _tag: 'Levels',
      levels: [
        [false, false],
        [true, false],
        [true, true],
        [false, true],
        [false, false],
      ],
    })
  })

  test('the hard reset plan only moves RTS', () => {
    expect(recognitionOf('hardReset')).toEqual({ _tag: 'RtsPulse' })
    expect(
      RESET_PLANS.hardReset.every(step => step.dataTerminalReady === undefined),
    ).toBe(true)
  })
})
