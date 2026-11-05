import * as Deferred from 'effect/Deferred'
import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Ref from 'effect/Ref'
import * as Stream from 'effect/Stream'

import { type Chip, type Region, type Session, md5Hex } from '@cycling/flasher'

export interface MemorySession {
  readonly session: Session
  /** How many times `write` ran far enough to touch flash. */
  readonly writes: Ref.Ref<number>
  /** Makes `session.lost` succeed, as an unplug does. */
  readonly unplug: Effect.Effect<void>
}

/** A C606's ESP32-S3 as the flasher describes it, for the fake page and tests. */
export const DEMO_CHIP: Chip = {
  mac: '020000000001',
  family: 'ESP32-S3',
  model: 'ESP32-S3 (QFN56)',
  revision: 'v0.2',
  features: ['Wi-Fi', 'BLE'],
  crystalMhz: 40,
  flash: { manufacturer: 0x20, device: 0x4018, bytes: 16 * 1024 * 1024 },
  usb: { vendorId: 0x303a, productId: 0x1001 },
  security: { _tag: 'Known', secureBoot: false, flashEncryption: false },
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
      bootloaderOffset: 0,
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
          Stream.concat(
            Stream.sync(() => ({
              _tag: 'Read' as const,
              md5: md5Hex(
                flash.subarray(region.address, region.address + region.size),
              ),
            })),
          ),
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
              md5Hex(
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
