import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Fiber from 'effect/Fiber'
import * as Stream from 'effect/Stream'

import { open } from '../src/index.ts'
import {
  consoleText,
  noise,
  portOf,
  session,
  simulator,
  valueOf,
} from './harness.ts'
import { FLASH_BYTES } from './sim/device.ts'

const sim = simulator()

describe('reboot and close', () => {
  test('reboot resets into the app; the re-enumerated port prints the boot log', async () => {
    const device = sim()
    const next = device.nextPort()
    const exit = await session(device, s =>
      s.reboot.pipe(
        Effect.andThen(s.lost),
        Effect.andThen(Effect.sync(() => device.state().mode)),
      ),
    )
    expect(valueOf(exit)).toBe('app')
    const text = await consoleText(await next, /uptime_ms/, 2000)
    expect(text).toContain('rst:0x10 (RTCWDT_RTC_RST)')
    expect(text).toContain('"uptime_ms"')
    expect(text).not.toContain('waiting for download')
  })

  test('closing the scope without reboot leaves the chip in the app and the port closed', async () => {
    const device = sim()
    const port = portOf(device)
    valueOf(await session(device, s => s.digest({ address: 0, size: 0x1000 })))
    expect(device.state()).toMatchObject({ mode: 'app', resets: 1 })
    expect(port.readable).toBeNull()
    expect(port.connected).toBe(false)
  })

  test('interrupting a read, then closing, still resets the chip into the app', async () => {
    const device = sim({ flash: noise(FLASH_BYTES, 8) })
    const exit = await Effect.runPromiseExit(
      Effect.scoped(
        Effect.gen(function* () {
          const s = yield* open(portOf(device), { entry: 'reset' })
          const reading = yield* Effect.forkChild(
            Stream.runDrain(s.read({ address: 0, size: FLASH_BYTES })),
          )
          yield* Effect.sleep(50)
          yield* Fiber.interrupt(reading)
        }),
      ),
    )
    valueOf(exit)
    expect(device.state()).toMatchObject({ mode: 'app', resets: 1 })
  })
})
