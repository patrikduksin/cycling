import * as Effect from 'effect/Effect'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import type * as NetAddress from 'effect/net/NetAddress'
import * as Socket from 'effect/socket/Socket'

import * as BunSocketServer from '@effect/platform-bun/BunSocketServer'

import { LocalResourceError } from './errors.ts'

/** One stream connection, open while the scope that opened it lives. */
export interface Connection {
  /** Single consumer. Ends when the peer closes. */
  readonly incoming: Stream.Stream<Uint8Array>
  /** Waits out the socket's backpressure; false once the socket is gone. */
  readonly write: (bytes: Uint8Array) => Effect.Effect<boolean>
}

export const connection = (
  socket: Socket.Socket,
): Effect.Effect<Connection, Socket.SocketError, Scope.Scope> =>
  Effect.gen(function* () {
    const pull = yield* Socket.readerBytes(socket)
    const writer = yield* socket.writer
    return {
      incoming: Stream.fromEffectRepeat(pull).pipe(
        Stream.flattenIterable,
        Stream.catchTag('SocketError', () => Stream.empty),
      ),
      write: bytes =>
        writer.write(bytes).pipe(
          Effect.as(true),
          Effect.orElseSucceed(() => false),
        ),
    }
  })

/**
 * A listener for the caller's scope: each accepted connection runs `handle`
 * in its own fiber and closes when `handle` ends.
 */
export const serve = <E>(options: {
  readonly listen:
    | { readonly path: string }
    | { readonly port: 0; readonly host: string }
  readonly resource: LocalResourceError['resource']
  readonly handle: (
    connection: Connection,
  ) => Effect.Effect<void, E, Scope.Scope>
}): Effect.Effect<NetAddress.SocketAddress, LocalResourceError, Scope.Scope> =>
  Effect.gen(function* () {
    const server = yield* BunSocketServer.make(options.listen).pipe(
      Effect.mapError(
        error =>
          new LocalResourceError({
            resource: options.resource,
            detail: String(error.reason.cause),
          }),
      ),
    )
    yield* Effect.forkScoped(
      server.run(socket =>
        connection(socket).pipe(
          Effect.flatMap(options.handle),
          Effect.scoped,
          Effect.ignore,
        ),
      ),
    )
    return server.address
  })
