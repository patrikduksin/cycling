import * as Effect from 'effect/Effect'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import {
  type Entry,
  FlasherError,
  type ReadEvent,
  type Session,
} from '@cycling/flasher'
import { Chip, MAX_DETAIL_CHARS, Md5, type Region } from '@cycling/protocol'

/**
 * A flasher session in the wire's types. `@cycling/flasher` knows nothing
 * of the protocol, so its chip facts and digests are decoded here, once, and
 * its error details are cut to the wire's limit here too.
 */
export type PageReadEvent =
  | Exclude<ReadEvent, { readonly _tag: 'Read' }>
  | { readonly _tag: 'Read'; readonly md5: Md5 }

export interface PageSession {
  readonly chip: Chip
  readonly read: (region: Region) => Stream.Stream<PageReadEvent, FlasherError>
  readonly write: Session['write']
  readonly digest: (region: Region) => Effect.Effect<Md5, FlasherError>
  readonly reboot: Session['reboot']
  readonly lost: Session['lost']
}

export type OpenFlasher = (
  port: SerialPort,
  options: { readonly entry: Entry },
) => Effect.Effect<Session, FlasherError, Scope.Scope>

const clipped = (error: FlasherError): FlasherError =>
  error.detail.length <= MAX_DETAIL_CHARS
    ? error
    : new FlasherError({
        reason: error.reason,
        detail: error.detail.slice(0, MAX_DETAIL_CHARS),
      })

const decodeChip = Schema.decodeUnknownEffect(Chip)

const decodeMd5 = (hex: string): Effect.Effect<Md5, FlasherError> =>
  Schema.decodeEffect(Md5)(hex).pipe(
    Effect.mapError(
      () =>
        new FlasherError({
          reason: 'chipError',
          detail: 'The chip sent a malformed digest.',
        }),
    ),
  )

/** The session in the wire's types; a chip whose facts do not parse cannot be entered. */
export const pageSession = (
  session: Session,
): Effect.Effect<PageSession, FlasherError> =>
  decodeChip(session.chip).pipe(
    Effect.mapError(
      () =>
        new FlasherError({
          reason: 'entryFailed',
          detail: 'The chip facts did not parse.',
        }),
    ),
    Effect.map((chip): PageSession => ({
      chip,
      read: region =>
        session.read(region).pipe(
          Stream.mapEffect(
            (event): Effect.Effect<PageReadEvent, FlasherError> =>
              event._tag === 'Read'
                ? Effect.map(decodeMd5(event.md5), md5 => ({
                    _tag: 'Read',
                    md5,
                  }))
                : Effect.succeed(event),
          ),
          Stream.mapError(clipped),
        ),
      write: (address, data) =>
        session.write(address, data).pipe(Stream.mapError(clipped)),
      digest: region =>
        session
          .digest(region)
          .pipe(Effect.flatMap(decodeMd5), Effect.mapError(clipped)),
      reboot: session.reboot.pipe(Effect.mapError(clipped)),
      lost: session.lost,
    })),
  )

export const openPage =
  (open: OpenFlasher) =>
  (
    port: SerialPort,
    options: { readonly entry: Entry },
  ): Effect.Effect<PageSession, FlasherError, Scope.Scope> =>
    open(port, options).pipe(
      Effect.flatMap(pageSession),
      Effect.mapError(clipped),
    )
