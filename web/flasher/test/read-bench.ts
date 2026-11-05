/**
 * How close a large read through `open(...).read` comes to the chip's own
 * rate, against the simulator delivering in Chrome's 255-byte reads.
 *
 *   bun web/flasher/test/read-bench.ts [MiB=4] [chip KB/s=140]
 *
 * A chip rate of 0 streams as fast as the host takes it, which measures
 * the host side alone.
 */
import * as Effect from 'effect/Effect'
import * as Stream from 'effect/Stream'

import { md5Hex, open } from '../src/index.ts'
import { FLASH_BYTES, SimDevice } from './sim/device.ts'

const mib = Number(process.argv[2] ?? 4)
const chipKbps = Number(process.argv[3] ?? 140)
const size = mib * 1024 * 1024

/** Noise, so a dropped or shifted byte cannot go unnoticed. */
const flash = new Uint8Array(FLASH_BYTES).map(
  (_, i) => (Math.imul(i, 0x9e3779b1) >>> 24) ^ (i >>> 12),
)

const device = new SimDevice({
  flash,
  readBytesPerSecond: chipKbps === 0 ? Infinity : chipKbps * 1000,
})
const port = device.current()
if (port === undefined) {
  throw new Error('the sim has no port enumerated')
}

const assembled = new Uint8Array(size)
const ms = await Effect.runPromise(
  Effect.scoped(
    Effect.gen(function* () {
      const session = yield* open(port, { entry: 'usbReset' })
      const started = performance.now()
      yield* Stream.runForEach(session.read({ address: 0, size }), event =>
        Effect.sync(() => {
          if (event._tag === 'Piece') {
            assembled.set(event.bytes, event.offset)
          }
        }),
      )
      return performance.now() - started
    }),
  ),
)
device.dispose()

const ok = md5Hex(assembled) === device.flashMd5(0, size)
const kbps = size / ms
const ideal =
  chipKbps === 0
    ? ''
    : `, ${((100 * kbps) / chipKbps).toFixed(0)}% of the chip's ${chipKbps} KB/s`
console.info(
  `${mib} MiB in ${(ms / 1000).toFixed(1)} s: ${kbps.toFixed(0)} KB/s${ideal}; bytes ${ok ? 'match' : 'DIFFER'}`,
)
