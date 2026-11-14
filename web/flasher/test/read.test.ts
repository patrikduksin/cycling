import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Stream from 'effect/Stream'

import { md5Hex, PIECE_RETRIES, READ_PIECE_BYTES } from '../src/index.ts'
import {
  noise,
  readAll,
  reasonOf,
  session,
  simulator,
  valueOf,
} from './harness.ts'
import { FLASH_BYTES } from './sim/device.ts'

const sim = simulator()

describe('read', () => {
  test('a chip whose replies come 30 ms late still enters and reads', async () => {
    const device = sim({ replyDelayMs: 30, flash: noise(FLASH_BYTES, 10) })
    const assembled = valueOf(
      await session(device, s => readAll(s, 0x8000, 0x10000)),
    )
    expect(md5Hex(assembled)).toBe(device.flashMd5(0x8000, 0x10000))
  })

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
