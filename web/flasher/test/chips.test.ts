import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Schedule from 'effect/Schedule'

import { md5Hex, READ_PIECE_BYTES } from '../src/index.ts'
import {
  consoleText,
  noise,
  portOf,
  readAll,
  session,
  simulator,
  valueOf,
  WATCHDOG_MS,
} from './harness.ts'
import { FLASH_BYTES, type SimOptions } from './sim/device.ts'

const sim = simulator()

const OTHER_CHIPS = [
  {
    chip: 'ESP32-C3',
    link: 'USB-Serial-JTAG',
    described: {
      family: 'ESP32-C3',
      model: 'ESP32-C3 (QFN32)',
      revision: 'v0.0',
      features: ['Wi-Fi', 'BLE'],
      usb: { vendorId: 0x303a, productId: 0x1001 },
      security: { _tag: 'Known', secureBoot: false, flashEncryption: false },
    },
    bootloader: 0,
    rebootCause: '0x10 (RTCWDT_RTC_RST)',
    enumerations: 2,
  },
  {
    chip: 'ESP32',
    link: 'a CP2102N UART bridge',
    described: {
      family: 'ESP32',
      model: 'ESP32-D0WDQ6',
      revision: '0',
      features: ['Wi-Fi', 'BT', 'Dual Core', 'Coding Scheme None'],
      usb: { vendorId: 0x10c4, productId: 0xea60 },
      security: { _tag: 'Unknown' },
    },
    bootloader: 0x1000,
    rebootCause: '0x1 (POWERON_RESET)',
    enumerations: 1,
  },
] as const

describe.each(OTHER_CHIPS)('an $chip on $link', expected => {
  const chipSim = (options: SimOptions = {}) =>
    sim({ chip: expected.chip, ...options })

  test('enters and describes itself, with its bootloader offset', async () => {
    const device = chipSim()
    const entered = valueOf(
      await session(device, s =>
        Effect.succeed({ chip: s.chip, bootloader: s.bootloaderOffset }),
      ),
    )
    expect(entered).toEqual({
      chip: {
        ...expected.described,
        mac: '020000123456',
        crystalMhz: 40,
        flash: { manufacturer: 0x20, device: 0x4018, bytes: FLASH_BYTES },
      },
      bootloader: expected.bootloader,
    })
  })

  test('reads a region that assembles to its flash and digests to the same MD5', async () => {
    const device = chipSim({ flash: noise(FLASH_BYTES, 20) })
    const size = READ_PIECE_BYTES + 0x1000
    const { assembled, digest } = valueOf(
      await session(device, s =>
        Effect.all({
          assembled: readAll(s, 0x8000, size),
          digest: s.digest({ address: 0x8000, size }),
        }),
      ),
    )
    expect(md5Hex(assembled)).toBe(device.flashMd5(0x8000, size))
    expect(digest).toBe(device.flashMd5(0x8000, size))
  })

  test('outlasts the RTC WDT a USB reset arms, or has none', async () => {
    const device = chipSim({ watchdogMs: WATCHDOG_MS })
    const exit = await session(device, s =>
      Effect.gen(function* () {
        const until = Date.now() + 2 * WATCHDOG_MS
        while (Date.now() < until) {
          yield* s.digest({ address: 0, size: 0x1000 })
        }
        return device.state()
      }),
    )
    expect(valueOf(exit)).toMatchObject({ mode: 'stub', resets: 0 })
  })

  test('reboots into the app the way python esptool does for its family and link', async () => {
    const device = chipSim()
    const rebooted =
      expected.enumerations === 1
        ? Promise.resolve(portOf(device))
        : device.nextPort()
    valueOf(
      await session(device, s =>
        s.reboot.pipe(
          Effect.andThen(
            Effect.repeat(
              Effect.sync(() => device.state().mode),
              {
                until: mode => mode === 'app',
                schedule: Schedule.spaced(20),
              },
            ),
          ),
        ),
      ),
    )
    const text = await consoleText(await rebooted, /uptime_ms/, 2000)
    expect(text).toContain('"uptime_ms"')
    expect(device.state()).toMatchObject({
      mode: 'app',
      lastReset: expected.rebootCause,
      enumerations: expected.enumerations,
    })
  })

  test('closing without a reboot still leaves the chip in its app', async () => {
    const device = chipSim()
    valueOf(await session(device, s => s.digest({ address: 0, size: 0x1000 })))
    expect(device.state()).toMatchObject({
      mode: 'app',
      lastReset: expected.rebootCause,
    })
  })
})
