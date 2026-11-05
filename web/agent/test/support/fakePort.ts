import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'

import {
  Baud,
  type Device,
  Epoch,
  type ResetKind,
  type SerialIdentity,
} from '@cycling/protocol'

import type { RemotePort, RemoteStatus } from '../../src/RemotePort.ts'

export const C606_IDENTITY: SerialIdentity = {
  vendorId: 0x303a,
  productId: 0x1001,
}
export const C606: Device = { _tag: 'serial', identity: C606_IDENTITY }
export const EPOCH_1 = Schema.decodeSync(Epoch)(1)
export const BAUD_115200 = Schema.decodeSync(Baud)(115200)

export const openStatus = (epoch: Epoch, baud: Baud): RemoteStatus => ({
  link: { _tag: 'Joined', browserPresent: true },
  device: Option.some(C606),
  port: Option.some({ epoch, state: { _tag: 'Open', baud } }),
  expiresAt: 0,
  traffic: { fromDevice: 0, toDevice: 0, dropped: 0 },
})

/** A RemotePort with no relay behind it: records what the doors asked of the device. */
export const fakePort = (status: RemoteStatus) =>
  Effect.gen(function* () {
    const resets = yield* Ref.make<ReadonlyArray<ResetKind>>([])
    const bauds = yield* Ref.make<ReadonlyArray<Baud>>([])
    const written = yield* Ref.make(0)
    const port: RemotePort = {
      device: Effect.succeed(C606),
      output: Stream.never,
      write: bytes =>
        Ref.update(written, n => n + bytes.length).pipe(Effect.as(true)),
      status: yield* SubscriptionRef.make(status),
      reset: kind => Ref.update(resets, all => [...all, kind]),
      setBaud: baud => Ref.update(bauds, all => [...all, baud]),
    }
    return {
      port,
      resets: Ref.get(resets),
      bauds: Ref.get(bauds),
      written: Ref.get(written),
    }
  })
