import * as Effect from 'effect/Effect'
import * as FileSystem from 'effect/FileSystem'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import { type Code, ResetKind } from '@cycling/protocol'
import * as BunSocket from '@effect/platform-bun/BunSocket'

import type { RemotePort } from './RemotePort.ts'
import { controlUsage, describeRequestError } from './Status.ts'
import { ControlRefused, type LocalResourceError } from './errors.ts'
import { connection, serve } from './sockets.ts'

const decodeKind = Schema.decodeUnknownOption(ResetKind)
const encoder = new TextEncoder()
const OK = 'ok'
const refusal = (text: string) => `error ${text}`

/** `reset <kind>` is the whole language; anything else is answered with an error line. */
export const parseRequest = (line: string): Option.Option<ResetKind> => {
  const [verb, kind, ...rest] = line.trim().split(/\s+/)
  return verb === 'reset' && rest.length === 0
    ? decodeKind(kind)
    : Option.none()
}

const answer = (port: RemotePort, line: string): Effect.Effect<string> =>
  Option.match(parseRequest(line), {
    onNone: () => Effect.succeed(refusal(controlUsage)),
    onSome: kind =>
      port.reset(kind).pipe(
        Effect.as(OK),
        Effect.catch(error =>
          Effect.succeed(refusal(describeRequestError(error))),
        ),
      ),
  })

/**
 * `<dir>/<CODE>/control.sock`: the explicit reset verb, for a tool that
 * opens the pty and so has no DTR/RTS for the recognizer to read.
 */
export const serveControl = (options: {
  readonly path: string
  readonly port: RemotePort
}): Effect.Effect<
  void,
  LocalResourceError,
  Scope.Scope | FileSystem.FileSystem
> =>
  Effect.gen(function* () {
    const { path, port } = options
    const fs = yield* FileSystem.FileSystem
    const remove = Effect.ignore(fs.remove(path, { force: true }))
    yield* Effect.acquireRelease(remove, () => remove)
    yield* serve({
      listen: { path },
      resource: 'control',
      handle: client =>
        client.incoming.pipe(
          Stream.decodeText,
          Stream.splitLines,
          Stream.runHead,
          Effect.flatMap(
            Option.match({
              onNone: () => Effect.void,
              onSome: line =>
                answer(port, line).pipe(
                  Effect.flatMap(reply =>
                    client.write(encoder.encode(`${reply}\n`)),
                  ),
                  Effect.asVoid,
                ),
            }),
          ),
        ),
    })
  })

/** The client side of the verb: `bun web/agent/src/main.ts reset <code> <kind>`. */
export const requestReset = (options: {
  readonly path: string
  readonly code: Code
  readonly kind: ResetKind
}): Effect.Effect<void, ControlRefused> =>
  Effect.scoped(
    Effect.gen(function* () {
      const { kind } = options
      const agent = yield* BunSocket.makeNet({ path: options.path })
      const client = yield* connection(agent).pipe(
        Effect.mapError(
          () =>
            new ControlRefused({
              kind,
              why: { _tag: 'NoAgent', code: options.code },
            }),
        ),
      )
      yield* client.write(encoder.encode(`reset ${kind}\n`))
      const reply = (yield* client.incoming.pipe(
        Stream.decodeText,
        Stream.mkString,
      )).trim()
      if (reply !== OK) {
        return yield* new ControlRefused({
          kind,
          why: { _tag: 'Answered', reply: reply.replace(/^error /, '') },
        })
      }
    }),
  )
