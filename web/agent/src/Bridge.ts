import * as Effect from 'effect/Effect'
import * as FileSystem from 'effect/FileSystem'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import { join } from 'node:path'

import type { Code } from '@cycling/protocol'

import { serveDeviceDoor } from './DeviceDoor.ts'
import { PtyHost } from './PtyHost.ts'
import type { RemotePort } from './RemotePort.ts'
import { LocalResourceError } from './errors.ts'

/** Where a session's local endpoints live, under the launcher's directory. */
export const sessionPaths = (dir: string, code: Code) => {
  const session = join(dir, code)
  return {
    session,
    port: join(session, 'port'),
    flasher: join(session, 'device.sock'),
  }
}

export interface Endpoints {
  /** `CYCLING_PORT`. */
  readonly pty: string
  /** `CYCLING_FLASHER`: the device socket. */
  readonly flasher: string
  /** Device bytes dropped because nothing read the pty. */
  readonly ptyDropped: Effect.Effect<number>
}

/**
 * The visitor's port as two local doors: a pty for console tools and the
 * device socket for flasher ops. Every OS resource it opens (the session
 * directory, the pty and its symlink, the socket) belongs to the caller's
 * scope and is removed in reverse order when it closes.
 */
export const open = (options: {
  readonly port: RemotePort
  readonly dir: string
  readonly code: Code
  readonly notice: (text: string) => Effect.Effect<void>
}): Effect.Effect<
  Endpoints,
  LocalResourceError,
  Scope.Scope | PtyHost | FileSystem.FileSystem
> =>
  Effect.gen(function* () {
    const { port } = options
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
    const output = yield* port.listen
    yield* Effect.forkScoped(output.pipe(Stream.runForEach(pty.write)))
    yield* Effect.forkScoped(pty.input.pipe(Stream.runForEach(port.write)))
    yield* serveDeviceDoor({
      path: paths.flasher,
      port,
      notice: options.notice,
    })
    return { pty: pty.path, flasher: paths.flasher, ptyDropped: pty.dropped }
  })
