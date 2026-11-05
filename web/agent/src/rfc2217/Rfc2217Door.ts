import * as Effect from 'effect/Effect'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'

import { Baud, type ResetKind, type SerialIdentity } from '@cycling/protocol'

import { openBaud, type RemotePort } from '../RemotePort.ts'
import { initialRecognizer, type Recognizer, step } from '../ResetRecognizer.ts'
import { LocalResourceError, type RequestError } from '../errors.ts'
import { type Connection, serve as listen } from '../sockets.ts'
import { type ClientRequest, greet, receive, type Session } from './ComPort.ts'
import { escapeData, initialParser, type Parser, parse } from './Telnet.ts'

/** What happened on the door, in order. */
export type Rfc2217Event =
  | { readonly _tag: 'Connected' }
  | { readonly _tag: 'Disconnected' }
  | { readonly _tag: 'Data'; readonly bytes: Uint8Array }
  | {
      readonly _tag: 'Reset'
      readonly kind: ResetKind
      readonly result: Option.Option<RequestError>
    }
  | { readonly _tag: 'Refused' }

export interface Rfc2217Door {
  /** `rfc2217://127.0.0.1:N`; this is `CYCLING_ESPTOOL_PORT`. */
  readonly url: string
  /** Single consumer. */
  readonly events: Stream.Stream<Rfc2217Event>
  /** Device bytes to the connected client. False when none is connected or its backlog is full. */
  readonly send: (bytes: Uint8Array) => Effect.Effect<boolean>
}

/** USB-Serial-JTAG ignores line coding; a browser reopen mid-flash would only cost a gap. */
const NATIVE_USB_SERIAL: ReadonlyArray<SerialIdentity> = [
  { vendorId: 0x303a, productId: 0x1001 },
]

export const ignoresBaud = (identity: SerialIdentity): boolean =>
  NATIVE_USB_SERIAL.some(
    native =>
      native.vendorId === identity.vendorId &&
      native.productId === identity.productId,
  )

const CLIENT_BACKLOG_CHUNKS = 256
const INITIAL_BAUD = 115200
const decodeBaud = Schema.decodeUnknownOption(Baud)

interface DoorState {
  readonly session: Session
  readonly parser: Parser
  readonly recognizer: Recognizer
}

/**
 * Loopback RFC 2217 server, one client at a time. Every telnet and
 * COM-PORT-OPTION exchange is answered here. Line commands go to the reset
 * recognizer and never reach the device as edges; a recognized sequence
 * becomes one `RemotePort.reset`. Purges are acknowledged and dropped,
 * because the relay is not a buffer.
 */
export const serve = (options: {
  readonly port: RemotePort
  readonly identity: SerialIdentity
  /** Sees every request a client makes, before the door acts on it. */
  readonly onRequest?: (request: ClientRequest) => Effect.Effect<void>
}): Effect.Effect<Rfc2217Door, LocalResourceError, Scope.Scope> =>
  Effect.gen(function* () {
    const { port, identity } = options
    const onRequest = options.onRequest ?? (() => Effect.void)
    const events = yield* Queue.unbounded<Rfc2217Event>()
    const emit = (event: Rfc2217Event) => Queue.offer(events, event)
    const client = yield* Ref.make(Option.none<Queue.Queue<Uint8Array>>())

    const request = (
      state: DoorState,
      outgoing: Queue.Queue<Uint8Array>,
      chunk: Uint8Array,
    ) =>
      Effect.gen(function* () {
        const parsed = parse(state.parser, chunk)
        let session = state.session
        let recognizer = state.recognizer
        for (const item of parsed.items) {
          const answered = receive(session, item)
          session = answered.session
          for (const reply of answered.reply) {
            yield* Queue.offer(outgoing, reply)
          }
          if (answered.request !== null) {
            yield* onRequest(answered.request)
            recognizer = yield* handle(answered.request, recognizer)
          }
        }
        return { session, parser: parsed.parser, recognizer }
      })

    const handle = (asked: ClientRequest, recognizer: Recognizer) =>
      Match.valueTags(asked, {
        Data: ({ bytes }) =>
          emit({ _tag: 'Data', bytes }).pipe(Effect.as(recognizer)),
        Purge: () => Effect.succeed(recognizer),
        SetBaud: ({ baud }) => changeBaud(baud).pipe(Effect.as(recognizer)),
        Line: ({ command }) => {
          const [next, kind] = step(recognizer, command)
          return (kind === null ? Effect.void : reset(kind)).pipe(
            Effect.as(next),
          )
        },
      })

    const reset = (kind: ResetKind) =>
      Effect.forkScoped(
        port.reset(kind).pipe(
          Effect.match({
            onSuccess: () => Option.none<RequestError>(),
            onFailure: Option.some,
          }),
          Effect.flatMap(result => emit({ _tag: 'Reset', kind, result })),
        ),
      )

    const changeBaud = (requested: number) =>
      Effect.gen(function* () {
        const baud = decodeBaud(requested)
        if (Option.isNone(baud) || ignoresBaud(identity)) return
        const current = openBaud(yield* SubscriptionRef.get(port.status))
        if (Option.contains(current, baud.value)) return
        yield* Effect.forkScoped(Effect.ignore(port.setBaud(baud.value)))
      })

    const attend = (connection: Connection) =>
      Effect.gen(function* () {
        const outgoing = yield* Queue.dropping<Uint8Array>(
          CLIENT_BACKLOG_CHUNKS,
        )
        const claimed = yield* Ref.modify(client, current =>
          Option.isSome(current)
            ? [false, current]
            : [true, Option.some(outgoing)],
        )
        if (!claimed) {
          yield* emit({ _tag: 'Refused' })
          return
        }
        yield* Effect.addFinalizer(() =>
          Ref.set(client, Option.none()).pipe(
            Effect.andThen(emit({ _tag: 'Disconnected' })),
          ),
        )
        yield* emit({ _tag: 'Connected' })
        yield* Effect.forkScoped(
          Effect.forever(
            Queue.take(outgoing).pipe(Effect.flatMap(connection.write)),
          ),
        )
        const greeting = greet(
          Option.getOrElse(
            openBaud(yield* SubscriptionRef.get(port.status)),
            () => INITIAL_BAUD,
          ),
        )
        for (const reply of greeting.reply) {
          yield* Queue.offer(outgoing, reply)
        }
        yield* connection.incoming.pipe(
          Stream.runFoldEffect(
            (): DoorState => ({
              session: greeting.session,
              parser: initialParser,
              recognizer: initialRecognizer,
            }),
            (state, chunk) => request(state, outgoing, chunk),
          ),
        )
      })

    const address = yield* listen({
      listen: { host: '127.0.0.1', port: 0 },
      resource: 'rfc2217',
      handle: attend,
    })
    if (address._tag === 'UnixPathAddress') {
      return yield* new LocalResourceError({
        resource: 'rfc2217',
        detail: `bound ${address.path}, not a TCP port`,
      })
    }
    return {
      url: `rfc2217://127.0.0.1:${address.port}`,
      events: Stream.fromQueue(events),
      send: (bytes: Uint8Array) =>
        Ref.get(client).pipe(
          Effect.flatMap(
            Option.match({
              onNone: () => Effect.succeed(false),
              onSome: outgoing => Queue.offer(outgoing, escapeData(bytes)),
            }),
          ),
        ),
    }
  })
