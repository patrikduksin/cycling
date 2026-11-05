import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import type { Entry } from '@cycling/flasher'
import type { Region, SerialIdentity } from '@cycling/protocol'

import { type PageSession, openPage } from '../adapters/flasher.ts'
import { type Adapters, EntryFailed, type PortLink } from '../links.ts'
import type { Layout } from '../model.ts'
import { REACQUIRE } from '../policy.ts'
import { makeInbox } from './inbox.ts'
import {
  PARTITION_TABLE,
  bootSlotOf,
  otadataOf,
  parsePartitions,
} from './partitions.ts'

/** Collects one region read from the session. */
const readAll = (session: PageSession, region: Region) =>
  session.read(region).pipe(
    Stream.runFold(
      () => new Uint8Array(region.size),
      (bytes, event) => {
        if (event._tag === 'Piece') {
          bytes.set(event.bytes, event.offset)
        }
        return bytes
      },
    ),
  )

/**
 * The device's own partition table and otadata, to name regions for the
 * visitor, and where its family keeps the bootloader. A device without a
 * table, or a read that fails, has its other regions shown by address.
 */
const readLayout = (session: PageSession): Effect.Effect<Layout> =>
  readAll(session, PARTITION_TABLE).pipe(
    Effect.map(parsePartitions),
    Effect.flatMap(partitions =>
      Option.match(otadataOf(partitions), {
        onNone: () => Effect.succeed(Option.none<string>()),
        onSome: region =>
          readAll(session, region).pipe(
            Effect.map(otadata => bootSlotOf(partitions, otadata)),
          ),
      }).pipe(Effect.map(bootSlot => ({ partitions, bootSlot }))),
    ),
    Effect.orElseSucceed(() => ({
      partitions: [],
      bootSlot: Option.none<string>(),
    })),
    Effect.map(found => ({ bootloader: session.bootloaderOffset, ...found })),
  )

/**
 * The flasher's side of the port: the granted port once it is back, a
 * session on it, the inbox for write payloads, and the device's own layout.
 * Any failure is an `EntryFailed` the holder table answers.
 */
export const acquireFlasher = (
  adapters: Pick<Adapters, 'serial' | 'flasher'>,
  options: { readonly identity: SerialIdentity; readonly entry: Entry },
): Effect.Effect<PortLink, EntryFailed, Scope.Scope> =>
  Effect.gen(function* () {
    const port = yield* adapters.serial
      .granted({ identity: options.identity, waitMs: REACQUIRE.timeoutMs })
      .pipe(
        Effect.mapError(
          () =>
            new EntryFailed({
              reason: 'entryFailed',
              detail: 'The device did not come back.',
            }),
        ),
      )
    const session = yield* openPage(adapters.flasher.open)(port, {
      entry: options.entry,
    }).pipe(
      Effect.mapError(
        ({ reason, detail }) =>
          new EntryFailed({
            reason:
              reason === 'portLost' || reason === 'unsupportedChip'
                ? reason
                : 'entryFailed',
            detail,
          }),
      ),
    )
    const inbox = yield* makeInbox
    const layout = yield* readLayout(session)
    return { _tag: 'Flasher', session, inbox, layout } satisfies PortLink
  })
