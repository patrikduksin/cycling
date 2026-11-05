import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as Socket from 'effect/socket/Socket'

import {
  decodeToAgent,
  decodeToBrowser,
  encodeAgentMsg,
  encodeBrowserMsg,
  type Frame,
  type ProtocolViolation,
  type Raw,
} from './frames.ts'
import type { Role } from './ids.ts'
import type { AgentMsg, BrowserMsg, ToAgent, ToBrowser } from './messages.ts'
import { KEEPALIVE, SUBPROTOCOL } from './wire.ts'

/** The socket closed; `code` is the relay's close code, 1006 when there was none. */
export interface Closed {
  readonly _tag: 'Closed'
  readonly code: number
}

/** Everything a client hears, in socket order; `Closed` is always last. */
export type RelayEvent<C> = Frame<C> | Closed

interface Side<In, Out> {
  readonly decode: (raw: Raw) => Effect.Effect<Frame<In>, ProtocolViolation>
  readonly encode: (message: Out) => string
}

interface Messages {
  readonly browser: { readonly receives: ToBrowser; readonly sends: BrowserMsg }
  readonly agent: { readonly receives: ToAgent; readonly sends: AgentMsg }
}

export type Receives<R extends Role> = Messages[R]['receives']
export type Sends<R extends Role> = Messages[R]['sends']

const SIDES: { readonly [R in Role]: Side<Receives<R>, Sends<R>> } = {
  browser: { decode: decodeToBrowser, encode: encodeBrowserMsg },
  agent: { decode: decodeToAgent, encode: encodeAgentMsg },
}

export interface RelaySocket<R extends Role> {
  /**
   * Decoded frames, then one `Closed`. Read it once: it is the socket's only
   * read side. A frame that fails to decode is dropped, not fatal.
   */
  readonly events: Stream.Stream<RelayEvent<Receives<R>>>
  /** False once the socket is closing: nothing is queued for a later connection. */
  readonly send: (item: Uint8Array | Sends<R>) => Effect.Effect<boolean>
}

export class RelayDialError extends Schema.TaggedError<
  RelayDialError,
  { readonly brand: unique symbol }
>()('RelayDialError', {
  reason: Schema.Literals(['unreachable', 'subprotocol']),
  detail: Schema.String,
}) {}

const OPEN = 1

const closeCode = (error: Socket.SocketError): number =>
  error.reason._tag === 'SocketCloseError' ? error.reason.code : 1006

const selectedProtocol = (ws: Socket.WebSocketLike): string =>
  'protocol' in ws && typeof ws.protocol === 'string' ? ws.protocol : ''

/**
 * One WebSocket to the relay for either role, on `effect/socket`. It is open
 * (and the relay chose `cycling.v1`) when this succeeds, pings every
 * `KEEPALIVE.intervalMs` while it lives, and closes with the scope. There is
 * no redial here: what a close means is `closeMeaning`'s, and each client
 * decides.
 */
export const relaySocket = <R extends Role>(options: {
  readonly role: R
  readonly url: string
  readonly protocols: ReadonlyArray<string>
  readonly openTimeout?: Duration.Input
}): Effect.Effect<
  RelaySocket<R>,
  RelayDialError,
  Scope.Scope | Socket.WebSocketConstructor
> =>
  Effect.gen(function* () {
    const side: Side<Receives<R>, Sends<R>> = SIDES[options.role]
    const construct = yield* Socket.WebSocketConstructor
    const ws = yield* Effect.acquireRelease(
      Effect.sync(() => construct(options.url, [...options.protocols])),
      created => Effect.sync(() => created.close(1000)),
    )
    const socket = yield* Socket.fromWebSocket(Effect.succeed(ws), {
      openTimeout: options.openTimeout ?? Duration.seconds(10),
    })
    const { pull } = yield* socket.reader.pipe(
      Effect.mapError(
        error =>
          new RelayDialError({ reason: 'unreachable', detail: error.message }),
      ),
    )
    const chosen = selectedProtocol(ws)
    if (chosen !== SUBPROTOCOL) {
      return yield* new RelayDialError({
        reason: 'subprotocol',
        detail: `relay chose subprotocol "${chosen}", not ${SUBPROTOCOL}`,
      })
    }
    const writer = yield* socket.writer
    const write = (data: string | Uint8Array): Effect.Effect<boolean> =>
      ws.readyState === OPEN
        ? writer.write(data).pipe(
            Effect.as(true),
            Effect.orElseSucceed(() => false),
          )
        : Effect.succeed(false)

    yield* Effect.forkScoped(
      Effect.forever(
        Effect.sleep(Duration.millis(KEEPALIVE.intervalMs)).pipe(
          Effect.andThen(write(KEEPALIVE.ping)),
        ),
      ),
    )

    const events: Stream.Stream<RelayEvent<Receives<R>>> =
      Stream.fromEffectRepeat(pull).pipe(
        Stream.flattenIterable,
        Stream.mapEffect(raw =>
          side.decode(raw).pipe(
            Effect.map((frame): ReadonlyArray<RelayEvent<Receives<R>>> => [
              frame,
            ]),
            Effect.catchTag('ProtocolViolation', violation =>
              Effect.logWarning('dropped a relay frame', violation.reason).pipe(
                Effect.as([]),
              ),
            ),
          ),
        ),
        Stream.flattenIterable,
        Stream.catchTag('SocketError', error =>
          Stream.make<ReadonlyArray<RelayEvent<Receives<R>>>>({
            _tag: 'Closed',
            code: closeCode(error),
          }),
        ),
      )

    return {
      events,
      send: item =>
        write(item instanceof Uint8Array ? item : side.encode(item)),
    }
  })
