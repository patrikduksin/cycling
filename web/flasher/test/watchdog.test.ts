import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import { ESPLoader, Transport } from 'esptool-js'

import { READ_PIECE_BYTES } from '../src/index.ts'
import {
  noise,
  portOf,
  readAll,
  session,
  simulator,
  valueOf,
  WATCHDOG_MS,
} from './harness.ts'
import { FLASH_BYTES } from './sim/device.ts'

const sim = simulator()

describe('the watchdog', () => {
  test('a session longer than the RTC WDT survives because open disabled it', async () => {
    const device = sim({
      watchdogMs: WATCHDOG_MS,
      flash: noise(FLASH_BYTES, 7),
    })
    const exit = await session(device, s =>
      Effect.gen(function* () {
        const until = Date.now() + 2 * WATCHDOG_MS
        while (Date.now() < until) {
          yield* readAll(s, 0, READ_PIECE_BYTES)
        }
        return device.state()
      }),
    )
    expect(valueOf(exit)).toMatchObject({ mode: 'stub', resets: 0 })
  })

  test('control: plain esptool-js without the disable writes is reset mid-session', async () => {
    const device = sim({ watchdogMs: WATCHDOG_MS })
    const port = portOf(device)
    const lost = new Promise<'lost'>(resolve =>
      port.addEventListener('disconnect', () => resolve('lost')),
    )
    const transport = new Transport(port, false)
    transport.trace = () => {}
    const loader = new ESPLoader({
      transport,
      baudrate: 115200,
      romBaudrate: 115200,
      terminal: { clean: () => {}, write: () => {}, writeLine: () => {} },
    })
    await loader.connect('usb_reset')
    await loader.runStub()
    const reads = (async () => {
      const until = Date.now() + 2 * WATCHDOG_MS
      while (Date.now() < until) {
        await loader.readReg(0x60008098)
      }
      return 'survived'
    })()
    expect(await Promise.race([lost, reads.catch(() => 'failed')])).toBe('lost')
    expect(device.state()).toMatchObject({ mode: 'app', resets: 1 })
    await transport.disconnect()
  })
})
