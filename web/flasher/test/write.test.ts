import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Fiber from 'effect/Fiber'
import * as Stream from 'effect/Stream'

import { md5Hex, open } from '../src/index.ts'
import { noise, portOf, session, simulator, valueOf } from './harness.ts'
import { OP } from './sim/device.ts'

const sim = simulator()

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
