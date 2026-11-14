import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Stream from 'effect/Stream'

import { open, redact } from '../src/index.ts'
import {
  noise,
  portOf,
  readAll,
  reasonOf,
  session,
  simulator,
  valueOf,
} from './harness.ts'
import { FLASH_BYTES } from './sim/device.ts'

const sim = simulator()

describe('open', () => {
  test('enters the stub and describes the chip', async () => {
    const device = sim({ secureBoot: true, flashCryptCnt: 0b111 })
    const entered = valueOf(
      await session(device, s =>
        Effect.succeed({ chip: s.chip, bootloader: s.bootloaderOffset }),
      ),
    )
    expect(entered).toEqual({
      chip: {
        mac: '020000123456',
        family: 'ESP32-S3',
        model: 'ESP32-S3 (QFN56)',
        revision: 'v0.0',
        features: ['Wi-Fi', 'BLE'],
        crystalMhz: 40,
        flash: { manufacturer: 0x20, device: 0x4018, bytes: FLASH_BYTES },
        usb: { vendorId: 0x303a, productId: 0x1001 },
        security: { _tag: 'Known', secureBoot: true, flashEncryption: true },
      },
      bootloader: 0,
    })
  })

  test('an even flash_crypt_cnt means flash encryption is off', async () => {
    const device = sim({ flashCryptCnt: 0b11 })
    const chip = valueOf(await session(device, s => Effect.succeed(s.chip)))
    expect(chip.security).toEqual({
      _tag: 'Known',
      secureBoot: false,
      flashEncryption: false,
    })
  })

  test('an entry that re-enumerates fails portLost, and noReset enters on the new port', async () => {
    const device = sim({ reenumerateOnEntry: true, reenumerateMs: 100 })
    const next = device.nextPort()
    const first = await session(device, s => Effect.succeed(s.chip))
    expect(reasonOf(first)).toBe('portLost')
    const port = await next
    const second = await Effect.runPromiseExit(
      Effect.scoped(
        open(port, { entry: 'noReset' }).pipe(Effect.map(s => s.chip.mac)),
      ),
    )
    expect(valueOf(second)).toBe('020000123456')
  })
})

describe('redaction', () => {
  test('MAC shapes become <mac> and the text is capped at 200 characters', () => {
    expect(
      redact('a 02:00:00:12:34:56 b 020000123456 c 02-00-00-12-34-56 d'),
    ).toBe('a <mac> b <mac> c <mac> d')
    expect(redact('x'.repeat(300))).toHaveLength(200)
    expect(redact('md5 0123456789abcdef0123456789abcdef')).toContain(
      '0123456789abcdef',
    )
  })

  test("a whole session's log never carries the MAC", async () => {
    const device = sim()
    const log = await Effect.runPromise(
      Effect.gen(function* () {
        const lines = yield* Effect.scoped(
          Effect.gen(function* () {
            const s = yield* open(portOf(device), { entry: 'reset' })
            yield* Stream.runDrain(s.write(0x10000, noise(0x8000, 9)))
            yield* readAll(s, 0, 0x2000)
            return s.log
          }),
        )
        return yield* Stream.runCollect(lines)
      }),
    )
    const text = log.join('\n')
    expect(text).toContain('Stub running')
    expect(text).not.toMatch(/02:00:00:12:34:56|020000123456/i)
  })
})
