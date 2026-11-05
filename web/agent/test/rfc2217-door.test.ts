import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Ref from 'effect/Ref'
import * as Stream from 'effect/Stream'

import type { SerialIdentity } from '@cycling/protocol'

import { serve } from '../src/rfc2217/Rfc2217Door.ts'
import {
  BAUD_115200,
  C606_IDENTITY,
  EPOCH_1,
  fakePort,
  openStatus,
} from './support/fakePort.ts'
import { rfcClient } from './support/rfcClient.ts'

const IAC = 0xff
const SB = 0xfa
const SE = 0xf0
const setBaud = (baud: number) => [
  IAC,
  SB,
  44,
  1,
  (baud >>> 24) & 0xff,
  (baud >>> 16) & 0xff,
  (baud >>> 8) & 0xff,
  baud & 0xff,
  IAC,
  SE,
]

const CP2102: SerialIdentity = { vendorId: 0x10c4, productId: 0xea60 }

const baudsForwarded = (identity: SerialIdentity) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.gen(function* () {
        const fake = yield* fakePort(openStatus(EPOCH_1, BAUD_115200))
        const door = yield* serve({ port: fake.port, identity })
        const client = yield* rfcClient(door.url)
        yield* client.send([...setBaud(115200), ...setBaud(921600)])
        yield* Effect.sleep('300 millis')
        return yield* fake.bauds
      }),
    ),
  )

describe('RFC 2217 SET_BAUDRATE', () => {
  test('a UART bridge gets a baud change, but not the baud it already runs at', async () => {
    expect(await baudsForwarded(CP2102)).toEqual([921600])
  })

  test('native USB-Serial-JTAG (303a:1001) acks and ignores every baud', async () => {
    expect(await baudsForwarded(C606_IDENTITY)).toEqual([])
  })
})

describe('RFC 2217 client slot (L4)', () => {
  test('of two near-simultaneous connects exactly one is served', () =>
    Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const fake = yield* fakePort(openStatus(EPOCH_1, BAUD_115200))
          const door = yield* serve({ port: fake.port, identity: CP2102 })
          const events = yield* Ref.make<ReadonlyArray<string>>([])
          yield* Effect.forkScoped(
            door.events.pipe(
              Stream.runForEach(event =>
                Ref.update(events, all => [...all, event._tag]),
              ),
            ),
          )
          const clients = yield* Effect.all(
            [rfcClient(door.url), rfcClient(door.url)],
            { concurrency: 'unbounded' },
          )
          yield* Effect.sleep('300 millis')
          const closed = yield* Effect.forEach(clients, client => client.closed)
          expect(closed.filter(Boolean)).toHaveLength(1)
          expect([...(yield* Ref.get(events))].sort()).toEqual([
            'Connected',
            'Refused',
          ])
        }),
      ),
    ))
})
