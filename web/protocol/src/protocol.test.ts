import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import {
  type AgentMsg,
  agentProtocols,
  assemble,
  assembled,
  type BrowserMsg,
  browserProtocols,
  Code,
  decodeFromAgent,
  decodeFromBrowser,
  decodeToAgent,
  decodeToBrowser,
  emptyAssembly,
  encodeAgentMsg,
  encodeBinary,
  encodeBrowserMsg,
  encodeRelayMsg,
  Epoch,
  formatCode,
  Lease,
  MAX_FLASH_BYTES,
  type Frame,
  MAX_FRAME_BYTES,
  mintCode,
  mintOwnerKey,
  OP_CHUNK_BYTES,
  opDataFrames,
  parseCode,
  parseOffered,
  type ProtocolViolation,
  RequestId,
  type RelayMsg,
} from './index.ts'
import * as ops from './ops.ts'

const run = <A, E>(effect: Effect.Effect<A, E>) => Effect.runSyncExit(effect)
const key = mintOwnerKey(new Uint8Array(16).fill(7))
const id = Schema.decodeSync(RequestId)(3)
const epoch = Schema.decodeSync(Epoch)(1)
const lease = Lease.make('pQ3-abcdefgh_123')
const md5 = '9e107d9d372bb6826bd81d3542a419d6'

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
  { _tag: 'PortChanged', status: { epoch, state: { _tag: 'Flashing' } } },
  { _tag: 'OpProgress', id, done: 524288, total: 1966080 },
  {
    _tag: 'OpResult',
    id,
    outcome: {
      _tag: 'Entered',
      chip: {
        mac: Schema.decodeSync(ops.Mac)('020000000001'),
        family: 'ESP32-S3',
        model: 'ESP32-S3 (QFN56)',
        revision: 'v0.2',
        features: ['Wi-Fi', 'BLE'],
        crystalMhz: 40,
        flash: { manufacturer: 0x20, device: 0x4018, bytes: 16 * 1024 * 1024 },
        usb: { vendorId: 0x303a, productId: 0x1001 },
        security: { _tag: 'Known', secureBoot: false, flashEncryption: false },
      },
    },
  },
  {
    _tag: 'OpResult',
    id,
    outcome: {
      _tag: 'Entered',
      chip: {
        mac: Schema.decodeSync(ops.Mac)('020000000002'),
        family: 'ESP32',
        model: 'ESP32-D0WD-V3',
        revision: '3',
        features: ['Wi-Fi', 'BT', 'Dual Core'],
        crystalMhz: 40,
        flash: { manufacturer: 0xc8, device: 0x4016, bytes: 4 * 1024 * 1024 },
        usb: null,
        security: { _tag: 'Unknown' },
      },
    },
  },
  {
    _tag: 'OpResult',
    id,
    outcome: { _tag: 'Written', md5: Schema.decodeSync(ops.Md5)(md5) },
  },
  {
    _tag: 'OpResult',
    id,
    outcome: { _tag: 'Failed', reason: 'transferGap', detail: '' },
  },
  { _tag: 'Bye' },
]
const region = { address: 0x760000, size: 0x1e0000 }
const agentMessages: ReadonlyArray<AgentMsg> = [
  { _tag: 'Op', id, op: { _tag: 'Enter', lease } },
  { _tag: 'Op', id, op: { _tag: 'Read', lease, region } },
  {
    _tag: 'Op',
    id,
    op: { _tag: 'Write', lease, region, md5: Schema.decodeSync(ops.Md5)(md5) },
  },
  { _tag: 'Op', id, op: { _tag: 'Reboot', lease } },
  { _tag: 'Op', id, op: { _tag: 'Release', lease } },
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
    const enter = encodeAgentMsg({
      _tag: 'Op',
      id,
      op: { _tag: 'Enter', lease },
    })
    expect(Exit.isFailure(run(decodeFromBrowser(enter)))).toBe(true)
    expect(Exit.isFailure(run(decodeToAgent(enter)))).toBe(true)
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

  test('an Entered chip outside the schema is refused, not stripped', () => {
    const entered = (chip: object) =>
      JSON.stringify({
        _tag: 'OpResult',
        id: 1,
        outcome: {
          _tag: 'Entered',
          chip: {
            mac: '020000000001',
            family: 'ESP32-C3',
            model: 'ESP32-C3 (QFN32)',
            revision: 'v0.4',
            features: ['Wi-Fi', 'BLE'],
            crystalMhz: 40,
            flash: { manufacturer: 0x20, device: 0x4016, bytes: 0x400000 },
            usb: { vendorId: 0x303a, productId: 0x1001 },
            security: { _tag: 'Unknown' },
            ...chip,
          },
        },
      })
    expect(Exit.isSuccess(run(decodeToAgent(entered({}))))).toBe(true)
    expect(
      Exit.isFailure(run(decodeToAgent(entered({ family: 'ESP31' })))),
    ).toBe(true)
    expect(
      Exit.isFailure(run(decodeToAgent(entered({ serial: 'AA:BB' })))),
    ).toBe(true)
    expect(
      Exit.isFailure(
        run(
          decodeToAgent(
            entered({ security: { _tag: 'Unknown', secureBoot: false } }),
          ),
        ),
      ),
    ).toBe(true)
  })

  test('an op cannot smuggle a MAC, or name flash past the chip', () => {
    const op = (extra: object) =>
      JSON.stringify({
        _tag: 'Op',
        id: 1,
        op: { _tag: 'Read', lease, ...extra },
      })
    expect(
      Exit.isSuccess(
        run(
          decodeFromAgent(
            op({ region: { address: 0, size: MAX_FLASH_BYTES } }),
          ),
        ),
      ),
    ).toBe(true)
    expect(
      Exit.isFailure(
        run(
          decodeFromAgent(
            op({ region: { address: 1, size: MAX_FLASH_BYTES } }),
          ),
        ),
      ),
    ).toBe(true)
    expect(
      Exit.isFailure(
        run(
          decodeFromAgent(
            op({ region: { address: 0, size: 16 }, mac: '020000000001' }),
          ),
        ),
      ),
    ).toBe(true)
  })

  test('console binary is bytes untouched behind channel 0, capped at MAX_FRAME_BYTES', () => {
    const bytes = new Uint8Array([0, 0xff, 0x11, 0x13, 0xc0])
    const frame = encodeBinary({ _tag: 'Console', bytes })
    expect(frame[0]).toBe(0)
    expect(run(decodeToAgent(frame.slice().buffer))).toEqual(
      Exit.succeed({ _tag: 'Console', bytes }),
    )
    const big = new Uint8Array(MAX_FRAME_BYTES + 1)
    expect(Exit.isFailure(run(decodeFromBrowser(big)))).toBe(true)
  })

  test('op data carries its id and offset, and an unknown channel or short header is malformed', () => {
    const bytes = new Uint8Array([1, 2, 3])
    const frame = encodeBinary({ _tag: 'OpData', id, offset: 0xabcdef, bytes })
    expect(run(decodeToBrowser(frame))).toEqual(
      Exit.succeed({ _tag: 'OpData', id, offset: 0xabcdef, bytes }),
    )
    expect(Exit.isFailure(run(decodeToBrowser(new Uint8Array([2, 0]))))).toBe(
      true,
    )
    expect(Exit.isFailure(run(decodeToBrowser(frame.subarray(0, 8))))).toBe(
      true,
    )
    expect(Exit.isFailure(run(decodeToBrowser(new Uint8Array(0))))).toBe(true)
  })
})

describe('op data', () => {
  const payload = new Uint8Array(OP_CHUNK_BYTES * 2 + 7).map((_, i) => i * 7)
  const frames = [...opDataFrames(id, payload)]

  test('every frame fits under the relay limit, and offsets run in order', () => {
    expect(frames.map(frame => frame.offset)).toEqual([
      0,
      OP_CHUNK_BYTES,
      OP_CHUNK_BYTES * 2,
    ])
    expect(
      frames.every(frame => encodeBinary(frame).byteLength <= MAX_FRAME_BYTES),
    ).toBe(true)
  })

  test('assembly puts the payload back together', () => {
    const whole = frames.reduce(
      (assembly, frame) => Option.getOrThrow(assemble(assembly, frame)),
      emptyAssembly(payload.byteLength),
    )
    expect(whole.received).toBe(payload.byteLength)
    expect(assembled(whole)).toEqual(payload)
  })

  test('assembly refuses a hole, an overlap and an overrun', () => {
    const [first, second, third] = frames
    if (first === undefined || second === undefined || third === undefined) {
      throw new Error('three frames expected')
    }
    const one = Option.getOrThrow(
      assemble(emptyAssembly(payload.length), first),
    )
    expect(Option.isNone(assemble(one, third))).toBe(true)
    expect(Option.isNone(assemble(one, first))).toBe(true)
    expect(
      Option.isNone(assemble(emptyAssembly(first.bytes.length - 1), first)),
    ).toBe(true)
    expect(Option.isSome(assemble(one, second))).toBe(true)
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
      entries: ['cycling.v3', 'auth.abc123'],
      version: true,
      token: Option.some('abc123'),
      ownerKey: Option.none(),
      selected: Option.some('cycling.v3'),
    })
    const browser = parseOffered(browserProtocols(key).join(','))
    expect(browser.ownerKey).toEqual(Option.some(key))
    expect(parseOffered('cycling.v4').version).toBe(false)
    expect(parseOffered(null).version).toBe(false)
    expect(parseOffered('cycling.v3, own.short').ownerKey).toEqual(
      Option.none(),
    )
  })

  test('the echo is the version token, else the first non-credential, never a credential', () => {
    const echo = (header: string | null) => parseOffered(header).selected
    expect(echo('auth.secret, cycling.v3')).toEqual(Option.some('cycling.v3'))
    expect(echo('auth.secret, cycling.v0')).toEqual(Option.some('cycling.v0'))
    expect(echo('auth.secret, own.key')).toEqual(Option.none())
    expect(echo(null)).toEqual(Option.none())
  })
})
