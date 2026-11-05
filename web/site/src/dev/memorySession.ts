import * as Deferred from 'effect/Deferred'
import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Ref from 'effect/Ref'
import * as Stream from 'effect/Stream'

import type { Chip, Region, Session } from '@cycling/flasher'

/**
 * Not MD5: four FNV-1a lanes printed as 32 hex digits. Tests and `?fake`
 * need a digest that is cheap and deterministic; the real one is
 * `md5Hex` from `@cycling/flasher`.
 */
export const fakeMd5Hex = (bytes: Uint8Array): string =>
  [0x811c_9dc5, 0x0100_0193, 0xdead_beef, 0x1234_5678]
    .map(seed => {
      let hash = seed
      for (const byte of bytes) {
        hash = Math.imul(hash ^ byte, 0x0100_0193) >>> 0
      }
      return hash.toString(16).padStart(8, '0')
    })
    .join('')

export interface MemorySession {
  readonly session: Session
  /** How many times `write` ran far enough to touch flash. */
  readonly writes: Ref.Ref<number>
  /** Makes `session.lost` succeed, as an unplug does. */
  readonly unplug: Effect.Effect<void>
}

/**
 * A session over a Uint8Array that takes the time a chip would: reads and
 * writes move `bytesPerSecond` (Infinity: no wait), a digest takes `digestMs`.
 */
export const memorySession = (options: {
  readonly flash: Uint8Array
  readonly chip: Chip
  readonly bytesPerSecond: number
  readonly digestMs: number
  readonly pieceBytes: number
}): Effect.Effect<MemorySession> =>
  Effect.gen(function* () {
    const { flash, bytesPerSecond, pieceBytes } = options
    const writes = yield* Ref.make(0)
    const lost = yield* Deferred.make<void>()
    const wait = (ms: number) =>
      ms > 0 ? Effect.sleep(Duration.millis(ms)) : Effect.void
    const pace = (bytes: number) => wait((bytes / bytesPerSecond) * 1000)

    const pieces = (region: Region) =>
      Stream.range(0, Math.ceil(region.size / pieceBytes) - 1).pipe(
        Stream.map(index => {
          const offset = index * pieceBytes
          return { offset, size: Math.min(pieceBytes, region.size - offset) }
        }),
      )

    const session: Session = {
      chip: options.chip,
      read: region =>
        pieces(region).pipe(
          Stream.mapEffect(({ offset, size }) =>
            pace(size).pipe(
              Effect.as([
                {
                  _tag: 'Piece' as const,
                  offset,
                  bytes: flash.slice(
                    region.address + offset,
                    region.address + offset + size,
                  ),
                },
                { _tag: 'Progress' as const, done: offset + size },
              ]),
            ),
          ),
          Stream.flattenIterable,
        ),
      write: (address, data) =>
        Stream.fromEffect(Ref.update(writes, n => n + 1)).pipe(
          Stream.drain,
          Stream.concat(
            pieces({ address, size: data.byteLength }).pipe(
              Stream.mapEffect(({ offset, size }) =>
                pace(size).pipe(
                  Effect.andThen(
                    Effect.sync(() => {
                      flash.set(
                        data.subarray(offset, offset + size),
                        address + offset,
                      )
                      return offset + size
                    }),
                  ),
                ),
              ),
            ),
          ),
        ),
      digest: region =>
        wait(options.digestMs).pipe(
          Effect.andThen(
            Effect.sync(() =>
              fakeMd5Hex(
                flash.subarray(region.address, region.address + region.size),
              ),
            ),
          ),
        ),
      reboot: Effect.void,
      log: Stream.empty,
      lost: Deferred.await(lost),
    }
    return { session, writes, unplug: Deferred.succeed(lost, undefined) }
  })
