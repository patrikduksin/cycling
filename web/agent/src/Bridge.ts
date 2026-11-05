import * as Effect from 'effect/Effect'
import * as FileSystem from 'effect/FileSystem'
import * as Match from 'effect/Match'
import * as Queue from 'effect/Queue'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'
import { join } from 'node:path'

import type { Code, SerialIdentity } from '@cycling/protocol'

import { serveControl } from './Control.ts'
import {
  type Delivery,
  type DoorEvent,
  type Doors,
  initialDoors,
  route,
} from './Doors.ts'
import { PtyHost } from './PtyHost.ts'
import type { RemotePort } from './RemotePort.ts'
import { LocalResourceError } from './errors.ts'
import { type Rfc2217Event, serve } from './rfc2217/Rfc2217Door.ts'

/** Where a session's local endpoints live, under the launcher's directory. */
export const sessionPaths = (dir: string, code: Code) => {
  const session = join(dir, code)
  return {
    session,
    port: join(session, 'port'),
    control: join(session, 'control.sock'),
  }
}

export interface Endpoints {
  /** `CYCLING_PORT`. */
  readonly pty: string
  /** `CYCLING_ESPTOOL_PORT`. */
  readonly rfc2217: string
  readonly doors: SubscriptionRef.SubscriptionRef<Doors>
  /** Device bytes dropped because nothing read the pty. */
  readonly ptyDropped: Effect.Effect<number>
  /** RFC 2217 happenings worth a line of output: resets and refused clients. */
  readonly notices: Stream.Stream<Rfc2217Event>
}

const doorEvent = (event: Rfc2217Event): ReadonlyArray<DoorEvent> =>
  Match.valueTags(event, {
    Connected: (): ReadonlyArray<DoorEvent> => [
      { _tag: 'Rfc2217Client', connected: true },
    ],
    Disconnected: (): ReadonlyArray<DoorEvent> => [
      { _tag: 'Rfc2217Client', connected: false },
    ],
    Data: ({ bytes }): ReadonlyArray<DoorEvent> => [
      { _tag: 'FromDoor', door: 'rfc2217', bytes },
    ],
    Reset: () => [],
    Refused: () => [],
  })

const isNotice = (event: Rfc2217Event) =>
  event._tag === 'Reset' || event._tag === 'Refused'

/**
 * One remote byte stream, two local doors. Every OS resource it opens (the
 * session directory, the pty and its symlink, the RFC 2217 listener, the
 * control socket) belongs to the caller's scope and is removed in reverse
 * order when it closes.
 */
export const open = (options: {
  readonly port: RemotePort
  readonly identity: SerialIdentity
  readonly dir: string
  readonly code: Code
}): Effect.Effect<
  Endpoints,
  LocalResourceError,
  Scope.Scope | PtyHost | FileSystem.FileSystem
> =>
  Effect.gen(function* () {
    const { port, identity } = options
    const fs = yield* FileSystem.FileSystem
    const paths = sessionPaths(options.dir, options.code)
    yield* Effect.acquireRelease(
      fs.makeDirectory(paths.session, { recursive: true, mode: 0o700 }).pipe(
        Effect.mapError(
          error =>
            new LocalResourceError({
              resource: 'sessionDir',
              detail: error.message,
            }),
        ),
      ),
      () =>
        fs.readDirectory(paths.session).pipe(
          Effect.flatMap(left =>
            left.length === 0
              ? fs.remove(paths.session, { recursive: true })
              : Effect.void,
          ),
          Effect.ignore,
        ),
    )
    const pty = yield* (yield* PtyHost).open(paths.port)
    const rfc2217 = yield* serve({ port, identity })
    yield* serveControl({ path: paths.control, port })

    const doors = yield* SubscriptionRef.make(initialDoors)
    const notices = yield* Queue.unbounded<Rfc2217Event>()
    const deliver = (delivery: Delivery | null): Effect.Effect<unknown> => {
      if (delivery === null) return Effect.void
      if (delivery._tag === 'ToDevice') return port.write(delivery.bytes)
      return delivery.door === 'pty'
        ? pty.write(delivery.bytes)
        : rfc2217.send(delivery.bytes)
    }
    const fromDoor = rfc2217.events.pipe(
      Stream.tap(event =>
        isNotice(event) ? Queue.offer(notices, event) : Effect.void,
      ),
      Stream.flatMap(event => Stream.fromIterable(doorEvent(event))),
    )
    yield* Effect.forkScoped(
      Stream.mergeAll(
        [
          port.output.pipe(
            Stream.map((bytes): DoorEvent => ({ _tag: 'FromDevice', bytes })),
          ),
          pty.input.pipe(
            Stream.map((bytes): DoorEvent => ({
              _tag: 'FromDoor',
              door: 'pty',
              bytes,
            })),
          ),
          fromDoor,
        ],
        { concurrency: 'unbounded' },
      ).pipe(
        Stream.runForEach(event =>
          SubscriptionRef.modify(doors, current => {
            const [next, delivery] = route(current, event)
            return [delivery, next]
          }).pipe(Effect.flatMap(deliver)),
        ),
      ),
    )
    return {
      pty: pty.path,
      rfc2217: rfc2217.url,
      doors,
      ptyDropped: pty.dropped,
      notices: Stream.fromQueue(notices),
    }
  })
