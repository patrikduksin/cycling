import * as Context from 'effect/Context'
import * as Deferred from 'effect/Deferred'
import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as PubSub from 'effect/PubSub'
import * as Ref from 'effect/Ref'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'

import {
  type AgentMsg,
  type Code,
  RequestId,
  type RequestOutcome,
  type ToAgent,
} from '@cycling/protocol'

import { type Inbound, RelayClient } from './RelayClient.ts'
import {
  applyMessage,
  firstMatch,
  isPortOpen,
  type Link,
  type RemotePort,
  type RemoteStatus,
  type Traffic,
} from './RemotePort.ts'
import {
  type AdmissionError,
  PortNotOpen,
  RelayDown,
  RequestFailed,
  type RequestError,
  RequestTimedOut,
} from './errors.ts'

/** A reset or a reopen finishes in well under a second at the browser; this is the "browser is gone" bound. */
export const REQUEST_TIMEOUT_MS = 10_000

export class Remote extends Context.Service<
  Remote,
  {
    /** Resolves once the relay has welcomed this agent; closing the scope leaves the session to the visitor. */
    readonly attach: (
      code: Code,
    ) => Effect.Effect<RemotePort, AdmissionError, Scope.Scope>
  }
>()('agent/Remote') {}

const initialStatus: RemoteStatus = {
  link: { _tag: 'Connecting' },
  device: Option.none(),
  port: Option.none(),
  expiresAt: 0,
  traffic: { fromDevice: 0, toDevice: 0, dropped: 0 },
}

type Answer = Deferred.Deferred<RequestOutcome, RelayDown>

/** Requests waiting for the browser's result, by id; ids are unique per connection. */
const pendingRequests = Effect.gen(function* () {
  const nextId = yield* Ref.make(1)
  const waiting = yield* Ref.make<ReadonlyMap<RequestId, Answer>>(new Map())
  const take = (id: RequestId) =>
    Ref.modify(waiting, all => {
      const rest = new Map(all)
      rest.delete(id)
      return [Option.fromNullishOr(all.get(id)), rest]
    })
  return {
    open: Effect.gen(function* () {
      const id = RequestId.make(yield* Ref.getAndUpdate(nextId, n => n + 1))
      const answer = yield* Deferred.make<RequestOutcome, RelayDown>()
      yield* Ref.update(waiting, all => new Map(all).set(id, answer))
      return { id, answer }
    }),
    settle: (id: RequestId, outcome: RequestOutcome) =>
      take(id).pipe(
        Effect.flatMap(
          Option.match({
            onNone: () => Effect.void,
            onSome: answer => Deferred.succeed(answer, outcome),
          }),
        ),
      ),
    forget: (id: RequestId) => Effect.asVoid(take(id)),
    failAll: Ref.getAndSet(waiting, new Map()).pipe(
      Effect.flatMap(all =>
        Effect.forEach(all.values(), answer =>
          Deferred.fail(answer, new RelayDown()),
        ),
      ),
    ),
  }
})

const count = (
  status: SubscriptionRef.SubscriptionRef<RemoteStatus>,
  field: keyof Traffic,
  bytes: number,
) =>
  SubscriptionRef.update(status, current => ({
    ...current,
    traffic: { ...current.traffic, [field]: current.traffic[field] + bytes },
  }))

export const layer: Layer.Layer<Remote, never, RelayClient> = Layer.effect(
  Remote,
  Effect.gen(function* () {
    const relay = yield* RelayClient

    const attach = Effect.fn('Remote.attach')(function* (code: Code) {
      const link = yield* relay.connect(code)
      const status = yield* SubscriptionRef.make(initialStatus)
      const output = yield* PubSub.unbounded<Uint8Array>()
      const pending = yield* pendingRequests

      const onControl = (message: ToAgent) =>
        SubscriptionRef.update(status, current =>
          applyMessage(current, message),
        ).pipe(
          Effect.andThen(
            message._tag === 'ResetResult' ||
              message._tag === 'LineCodingResult'
              ? pending.settle(message.id, message.outcome)
              : Effect.void,
          ),
        )

      const setLink = (next: Link) =>
        SubscriptionRef.update(status, current => ({ ...current, link: next }))

      const onInbound = (event: Inbound) =>
        Match.valueTags(event, {
          Bytes: ({ bytes }) =>
            PubSub.publish(output, bytes).pipe(
              Effect.andThen(count(status, 'fromDevice', bytes.length)),
            ),
          Control: ({ message }) => onControl(message),
          Pong: () => Effect.void,
          Down: () =>
            setLink({ _tag: 'Redialing' }).pipe(
              Effect.andThen(pending.failAll),
            ),
          Stopped: ({ ending }) =>
            setLink({ _tag: 'Over', ending }).pipe(
              Effect.andThen(pending.failAll),
            ),
        })

      yield* Effect.forkScoped(link.inbound.pipe(Stream.runForEach(onInbound)))
      yield* firstMatch(status, current =>
        current.link._tag === 'Connecting' ? Option.none() : Option.some(true),
      )

      const request = (
        message: (id: RequestId) => AgentMsg,
      ): Effect.Effect<void, RequestError> =>
        Effect.gen(function* () {
          if ((yield* SubscriptionRef.get(status)).link._tag !== 'Joined') {
            return yield* new RelayDown()
          }
          const { id, answer } = yield* pending.open
          const outcome = yield* Effect.gen(function* () {
            if (!(yield* link.send(message(id)))) {
              return yield* new RelayDown()
            }
            return yield* Deferred.await(answer).pipe(
              Effect.timeoutOrElse({
                duration: REQUEST_TIMEOUT_MS,
                orElse: () =>
                  Effect.fail(new RequestTimedOut({ ms: REQUEST_TIMEOUT_MS })),
              }),
            )
          }).pipe(Effect.ensuring(pending.forget(id)))
          if (outcome === 'portNotOpen') return yield* new PortNotOpen()
          if (outcome === 'failed') return yield* new RequestFailed()
        })

      const write = (bytes: Uint8Array) =>
        Effect.gen(function* () {
          const open = isPortOpen(yield* SubscriptionRef.get(status))
          const sent = open && (yield* link.send(bytes))
          yield* count(status, sent ? 'toDevice' : 'dropped', bytes.length)
          return sent
        })

      return {
        device: firstMatch(status, current => current.device),
        output: Stream.fromPubSub(output),
        write,
        status,
        reset: kind => request(id => ({ _tag: 'Reset', id, kind })),
        setBaud: baud => request(id => ({ _tag: 'SetLineCoding', id, baud })),
      } satisfies RemotePort
    })

    return Remote.of({ attach })
  }),
)
