import * as Context from 'effect/Context'
import * as Deferred from 'effect/Deferred'
import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Redacted from 'effect/Redacted'
import * as Ref from 'effect/Ref'
import * as Schedule from 'effect/Schedule'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as Socket from 'effect/socket/Socket'

import {
  type AgentMsg,
  agentProtocols,
  backoffMs,
  CloseCode,
  closeMeaning,
  coalesceBytes,
  type Code,
  type Frame,
  type Refusal,
  relaySocket,
  type RelaySocket,
  sessionPath,
  type ToAgent,
} from '@cycling/protocol'

import { AgentConfig } from './AgentConfig.ts'
import {
  type AdmissionError,
  type Ending,
  RelayUnreachable,
  SessionNotFound,
  Unauthorized,
  UnsupportedVersion,
} from './errors.ts'

/** What the relay connection reports, in socket order: a `Down` before each redial, at most one `Stopped` last. */
export type Inbound =
  | Frame<ToAgent>
  | { readonly _tag: 'Down' }
  | { readonly _tag: 'Stopped'; readonly ending: Ending }

export interface RelayLink {
  readonly inbound: Stream.Stream<Inbound>
  /** False when the socket is down: bytes sent while redialing are dropped, never queued. */
  readonly send: (item: Uint8Array | AgentMsg) => Effect.Effect<boolean>
}

/**
 * The only place the token and the socket meet. One live WebSocket; on a
 * transient drop it redials on the protocol's backoff until a close code
 * says the session is over.
 */
export class RelayClient extends Context.Service<
  RelayClient,
  {
    readonly connect: (
      code: Code,
    ) => Effect.Effect<RelayLink, AdmissionError, Scope.Scope>
  }
>()('agent/RelayClient') {}

const rejected = (error: AdmissionError): Ending => ({
  _tag: 'Rejected',
  error,
})

const REFUSALS: { readonly [W in Refusal]: () => AdmissionError } = {
  unauthorized: () => new Unauthorized(),
  notFound: () => new SessionNotFound(),
  unsupportedVersion: () =>
    new UnsupportedVersion({ why: 'unsupportedVersion' }),
  protocolViolation: () => new UnsupportedVersion({ why: 'protocolViolation' }),
  rateLimited: () =>
    new RelayUnreachable({
      why: { _tag: 'Closed', code: CloseCode.rateLimited },
    }),
}

/** What a close tells an agent: None to redial, or how its session stopped. Codes are the browser's to mint, so `Remint` is a refusal. */
export const endingOf = (code: number): Option.Option<Ending> =>
  Match.valueTags(closeMeaning(code), {
    Transient: () => Option.none<Ending>(),
    Ended: ({ reason }) => Option.some<Ending>({ _tag: 'Ended', reason }),
    Replaced: () => Option.some<Ending>({ _tag: 'Replaced' }),
    Refused: ({ why }) => Option.some(rejected(REFUSALS[why]())),
    Remint: () => Option.some(rejected(new SessionNotFound())),
  })

/** Why the first socket did not get this agent in. */
const admissionOf = (ending: Ending): AdmissionError =>
  Match.valueTags(ending, {
    Rejected: ({ error }): AdmissionError => error,
    Ended: () => new SessionNotFound(),
    Replaced: () =>
      new RelayUnreachable({
        why: { _tag: 'Closed', code: CloseCode.replaced },
      }),
  })

/** A transient drop; `welcomed` means that socket was good, so the backoff starts over. */
export interface Dropped {
  readonly _tag: 'Dropped'
  readonly welcomed: boolean
}

/** Restarts after a drop that followed a Welcome. */
export const redialSchedule: Schedule.Schedule<Duration.Duration, Dropped> =
  Schedule.fromStep(
    Effect.sync(() => {
      let failures = 0
      return (_now: number, dropped: Dropped) => {
        failures = dropped.welcomed ? 1 : failures + 1
        const delay = Duration.millis(backoffMs(failures))
        const step: [Duration.Duration, Duration.Duration] = [delay, delay]
        return Effect.succeed(step)
      }
    }),
  )

export const layer: Layer.Layer<RelayClient, never, AgentConfig> = Layer.effect(
  RelayClient,
  Effect.gen(function* () {
    const config = yield* AgentConfig
    const construct = yield* Socket.WebSocketConstructor
    const protocols = agentProtocols(Redacted.value(config.token))

    const connect = Effect.fn('RelayClient.connect')(function* (code: Code) {
      const url = `${config.relay}${sessionPath('agent', code)}`
      const inbound = yield* Queue.unbounded<Inbound>()
      const outbound = yield* Queue.unbounded<Uint8Array | AgentMsg>()
      const current = yield* Ref.make(Option.none<RelaySocket<'agent'>>())
      const joined = yield* Deferred.make<void, AdmissionError>()

      const onFrame = (frame: Frame<ToAgent>) => {
        const welcome =
          frame._tag === 'Control' && frame.message._tag === 'Welcome'
        return (
          welcome ? Deferred.succeed(joined, undefined) : Effect.void
        ).pipe(Effect.andThen(Queue.offer(inbound, frame)), Effect.as(welcome))
      }

      /** One socket from dial to close: its close code, and whether the relay welcomed it. */
      const connection = Effect.scoped(
        Effect.gen(function* () {
          const socket = yield* relaySocket({ role: 'agent', url, protocols })
          yield* Effect.acquireRelease(
            Ref.set(current, Option.some(socket)),
            () => Ref.set(current, Option.none()),
          )
          return yield* socket.events.pipe(
            Stream.runFoldEffect(
              () => ({ code: 1006, welcomed: false }),
              (seen, event) =>
                event._tag === 'Closed'
                  ? Effect.succeed({ ...seen, code: event.code })
                  : onFrame(event).pipe(
                      Effect.map(welcome => ({
                        ...seen,
                        welcomed: seen.welcomed || welcome,
                      })),
                    ),
            ),
          )
        }),
      ).pipe(Effect.provideService(Socket.WebSocketConstructor, construct))

      /** How one socket ended. Only a session this agent was let into is redialed. */
      const attempt: Effect.Effect<Ending, Dropped> = Effect.gen(function* () {
        const result = yield* Effect.result(connection)
        const admitted = yield* Deferred.isDone(joined)
        if (result._tag === 'Failure') {
          const { reason, detail } = result.failure
          return admitted
            ? yield* Effect.fail<Dropped>({ _tag: 'Dropped', welcomed: false })
            : rejected(
                new RelayUnreachable({ why: { _tag: 'Dial', reason, detail } }),
              )
        }
        const { code, welcomed } = result.success
        const ending = endingOf(code)
        if (Option.isSome(ending)) return ending.value
        return admitted
          ? yield* Effect.fail<Dropped>({ _tag: 'Dropped', welcomed })
          : rejected(new RelayUnreachable({ why: { _tag: 'Closed', code } }))
      })

      const supervise = attempt.pipe(
        Effect.tapError(() => Queue.offer(inbound, { _tag: 'Down' })),
        Effect.retry(redialSchedule),
        Effect.orDie,
        Effect.flatMap(ending =>
          Deferred.isDone(joined).pipe(
            Effect.flatMap(admitted =>
              admitted
                ? Queue.offer(inbound, { _tag: 'Stopped', ending })
                : Deferred.fail(joined, admissionOf(ending)),
            ),
          ),
        ),
      )

      const sender = Stream.fromQueue(outbound).pipe(
        coalesceBytes(),
        Stream.runForEach(item =>
          Ref.get(current).pipe(
            Effect.flatMap(
              Option.match({
                onNone: () => Effect.void,
                onSome: socket => socket.send(item),
              }),
            ),
          ),
        ),
      )

      yield* Effect.forkScoped(supervise)
      yield* Effect.forkScoped(sender)
      yield* Deferred.await(joined)
      return {
        inbound: Stream.fromQueue(inbound),
        send: (item: Uint8Array | AgentMsg) =>
          Ref.get(current).pipe(
            Effect.flatMap(socket =>
              Option.isSome(socket)
                ? Queue.offer(outbound, item)
                : Effect.succeed(false),
            ),
          ),
      } satisfies RelayLink
    })

    return RelayClient.of({ connect })
  }),
).pipe(Layer.provide(Socket.layerWebSocketConstructorGlobal))
