import { afterEach, describe, expect, setDefaultTimeout, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Fiber from 'effect/Fiber'
import * as Schedule from 'effect/Schedule'
import * as Stream from 'effect/Stream'
import { ESPLoader, Transport } from 'esptool-js'

import {
  FlasherError,
  md5Hex,
  open,
  PIECE_RETRIES,
  READ_PIECE_BYTES,
  redact,
  type Session,
} from '../src/index.ts'
import { FLASH_BYTES, OP, SimDevice, type SimOptions } from './sim/device.ts'
import type { FakeSerialPort } from './sim/port.ts'

// The simulator runs on the same event loop as esptool-js, so a loaded machine (cycling test runs
// the Rust and Python suites beside this one) stretches every case well past bun's 5 s default.
setDefaultTimeout(30_000)

const MAC = '02:00:00:12:34:56'

const devices: Array<SimDevice> = []
afterEach(() => {
  for (const device of devices.splice(0)) {
    device.dispose()
  }
})

/** Deterministic noise, so a dropped or shifted byte cannot go unnoticed. */
const noise = (length: number, seed: number): Uint8Array => {
  const words = new Uint32Array(Math.ceil(length / 4))
  let state = seed >>> 0 || 1
  for (let i = 0; i < words.length; i++) {
    state ^= state << 13
    state ^= state >>> 17
    state ^= state << 5
    words[i] = state >>> 0
  }
  return new Uint8Array(words.buffer, 0, length)
}

const sim = (options: SimOptions = {}): SimDevice => {
  const device = new SimDevice({ mac: MAC, ...options })
  devices.push(device)
  return device
}

const portOf = (device: SimDevice): FakeSerialPort => {
  const port = device.current()
  if (port === undefined) {
    throw new Error('the sim has no port enumerated')
  }
  return port
}

const session = <A, E>(
  device: SimDevice,
  use: (session: Session) => Effect.Effect<A, E>,
): Promise<Exit.Exit<A, E | FlasherError>> =>
  Effect.runPromiseExit(
    Effect.scoped(
      open(portOf(device), { entry: 'reset' }).pipe(Effect.flatMap(use)),
    ),
  )

const readAll = (
  session: Session,
  address: number,
  size: number,
): Effect.Effect<Uint8Array, FlasherError> =>
  Effect.gen(function* () {
    const assembled = new Uint8Array(size)
    yield* Stream.runForEach(session.read({ address, size }), event =>
      Effect.sync(() => {
        if (event._tag === 'Piece') {
          assembled.set(event.bytes, event.offset)
        }
      }),
    )
    return assembled
  })

const reasonOf = (exit: Exit.Exit<unknown, unknown>): string | undefined =>
  Exit.isFailure(exit) && exit.cause.reasons[0]?._tag === 'Fail'
    ? exit.cause.reasons[0].error instanceof FlasherError
      ? exit.cause.reasons[0].error.reason
      : 'not a FlasherError'
    : undefined

const valueOf = <A>(exit: Exit.Exit<A, unknown>): A => {
  if (Exit.isSuccess(exit)) {
    return exit.value
  }
  throw new Error(`the session failed: ${String(exit.cause)}`)
}

/** Reads what the port prints until `until` matches or `ms` passes. */
const consoleText = async (
  port: SerialPort,
  until: RegExp,
  ms: number,
): Promise<string> => {
  await port.open({ baudRate: 115200 })
  const readable = port.readable
  if (readable === null) {
    throw new Error('the reopened port has no readable')
  }
  const reader = readable.getReader()
  const decoder = new TextDecoder()
  let text = ''
  const deadline = Date.now() + ms
  while (!until.test(text) && Date.now() < deadline) {
    const chunk = await Promise.race([
      reader.read(),
      new Promise<undefined>(resolve =>
        setTimeout(resolve, Math.max(0, deadline - Date.now())),
      ),
    ])
    if (chunk?.value !== undefined) {
      text += decoder.decode(chunk.value)
    }
  }
  await reader.cancel()
  reader.releaseLock()
  await port.close()
  return text
}

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

describe('read', () => {
  test('a chip whose replies come 30 ms late still enters and reads', async () => {
    const device = sim({ replyDelayMs: 30, flash: noise(FLASH_BYTES, 10) })
    const assembled = valueOf(
      await session(device, s => readAll(s, 0x8000, 0x10000)),
    )
    expect(md5Hex(assembled)).toBe(device.flashMd5(0x8000, 0x10000))
  })

  test('a full 16 MiB read assembles to the flash, its MD5 and the chip digest', async () => {
    const device = sim({ flash: noise(FLASH_BYTES, 1) })
    const started = Date.now()
    const { assembled, digest } = valueOf(
      await session(device, s =>
        Effect.all({
          assembled: readAll(s, 0, FLASH_BYTES),
          digest: s.digest({ address: 0, size: FLASH_BYTES }),
        }),
      ),
    )
    console.info(
      `16 MiB read through esptool-js and the sim: ${Date.now() - started} ms`,
    )
    const expected = device.flashMd5(0, FLASH_BYTES)
    expect(md5Hex(assembled)).toBe(expected)
    expect(digest).toBe(expected)
  }, 60_000)

  test('emits each verified piece, then its progress, in order', async () => {
    const device = sim({ flash: noise(FLASH_BYTES, 2) })
    const size = READ_PIECE_BYTES * 2 + 100
    const events = valueOf(
      await session(device, s =>
        Stream.runCollect(s.read({ address: 0x1000, size })),
      ),
    )
    expect(
      events.map(event =>
        event._tag === 'Piece'
          ? `piece ${event.offset} ${event.bytes.length}`
          : event._tag === 'Progress'
            ? `progress ${event.done}`
            : `read ${event.md5}`,
      ),
    ).toEqual([
      `piece 0 ${READ_PIECE_BYTES}`,
      `progress ${READ_PIECE_BYTES}`,
      `piece ${READ_PIECE_BYTES} ${READ_PIECE_BYTES}`,
      `progress ${READ_PIECE_BYTES * 2}`,
      `piece ${READ_PIECE_BYTES * 2} 100`,
      `progress ${size}`,
      `read ${device.flashMd5(0x1000, size)}`,
    ])
  })

  test('a piece corrupted once in transit is read again and the read succeeds', async () => {
    const device = sim({ flash: noise(FLASH_BYTES, 3) })
    device.corruptReads(0x123456, 1, 1)
    const assembled = valueOf(
      await session(device, s => readAll(s, 0x100000, 0x200000)),
    )
    expect(md5Hex(assembled)).toBe(device.flashMd5(0x100000, 0x200000))
  })

  test('a piece corrupted on every try fails digestMismatch', async () => {
    const device = sim({ flash: noise(FLASH_BYTES, 4) })
    device.corruptReads(0x123456, 1, PIECE_RETRIES + 1)
    const exit = await session(device, s => readAll(s, 0x100000, 0x200000))
    expect(reasonOf(exit)).toBe('digestMismatch')
  })

  test('unplugging mid-read fails portLost and the scope closes promptly', async () => {
    const device = sim({ flash: noise(FLASH_BYTES, 5) })
    let unpluggedAt = 0
    const exit = await session(device, s =>
      Stream.runForEach(s.read({ address: 0, size: FLASH_BYTES }), event =>
        Effect.sync(() => {
          if (event._tag === 'Piece' && unpluggedAt === 0) {
            device.unplug()
            unpluggedAt = Date.now()
          }
        }),
      ),
    )
    const closedAfter = Date.now() - unpluggedAt
    expect(reasonOf(exit)).toBe('portLost')
    expect(closedAfter).toBeLessThan(1000)
    expect(device.state().mode).toBe('unplugged')
  })
})

describe('write', () => {
  test('2 MiB at 0x410000 lands in flash and digests to the data', async () => {
    const device = sim()
    const data = noise(2 * 1024 * 1024, 6)
    const { progress, digest } = valueOf(
      await session(device, s =>
        Effect.all({
          progress: Stream.runCollect(s.write(0x410000, data)),
          digest: s.digest({ address: 0x410000, size: data.length }),
        }),
      ),
    )
    expect(digest).toBe(md5Hex(data))
    expect(
      md5Hex(device.flash.subarray(0x410000, 0x410000 + data.length)),
    ).toBe(md5Hex(data))
    expect(progress.at(-1)).toBe(data.length)
    expect(
      [...progress].every(
        (done, i) => i === 0 || done >= (progress[i - 1] ?? 0),
      ),
    ).toBe(true)
  })
})

/** A short RTC WDT, so the watchdog cases take about a second of wall clock. */
const WATCHDOG_MS = 500

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

/** Each family on the link its boards use, entered, described, read and rebooted as the S3 is above. */
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

describe('regions', () => {
  test('a 4 MiB chip refuses regions past its flash and leaves flash untouched', async () => {
    const flash = new Uint8Array(4 * 1024 * 1024).fill(0xff)
    const device = sim({ flash })
    const pastEnd = { address: 0x3f0000, size: 0x20000 }
    const reasons = valueOf(
      await session(device, s =>
        Effect.forEach(
          [
            Stream.runDrain(s.write(pastEnd.address, noise(pastEnd.size, 12))),
            Stream.runDrain(s.read(pastEnd)),
            s.digest(pastEnd),
            s.digest({ address: 0x3f0000, size: 0x10000 }),
          ],
          effect =>
            effect.pipe(
              Effect.flip,
              Effect.map(error => error.reason),
              Effect.orElseSucceed(() => 'ok'),
            ),
        ),
      ),
    )
    expect(reasons).toEqual(['pastFlash', 'pastFlash', 'pastFlash', 'ok'])
    expect(flash.every(byte => byte === 0xff)).toBe(true)
  })
})

describe('an abandoned write', () => {
  test('stops at a block boundary before the recovery reset, which never interleaves with it', async () => {
    const device = sim({ replyDelayMs: 30 })
    const data = noise(4 * 1024 * 1024, 11)
    const closed = await Effect.runPromiseExit(
      Effect.scoped(
        Effect.gen(function* () {
          const s = yield* open(portOf(device), { entry: 'reset' })
          const writing = yield* Effect.forkChild(
            Stream.runDrain(s.write(0x400000, data)),
          )
          yield* Effect.sleep(300)
          yield* Fiber.interrupt(writing)
          return Date.now()
        }),
      ),
    )
    const closedAfter = Date.now() - valueOf(closed)
    const commands = device.commands()
    const afterWrite = commands.slice(commands.lastIndexOf(OP.FLASH_DEFL_BEGIN))
    const recovery = afterWrite.indexOf(OP.WRITE_REG)
    expect(recovery).toBeGreaterThan(0)
    expect(afterWrite.slice(recovery)).not.toContain(OP.FLASH_DEFL_DATA)
    expect(closedAfter).toBeLessThan(2000)
    expect(device.state()).toMatchObject({ mode: 'app', resets: 1 })
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
