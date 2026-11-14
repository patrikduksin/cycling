import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'

import { md5Hex } from '../src/index.ts'
import { noise, readAll, session, simulator, valueOf } from './harness.ts'
import { FLASH_BYTES } from './sim/device.ts'

const sim = simulator()

describe('a full read', () => {
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
})
