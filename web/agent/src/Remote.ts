import type * as Cause from 'effect/Cause'
import * as Context from 'effect/Context'
import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as PubSub from 'effect/PubSub'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'

import {
  type Code,
  type Op,
  OP_STALL_MS,
  opDataFrames,
  RequestId,
  type ToAgent,
} from '@cycling/protocol'

import { type Inbound, RelayClient } from './RelayClient.ts'
import {
  applyMessage,
  firstMatch,
  isPortOpen,
  type Link,
  type OpEvent,
  type RemotePort,
  type RemoteStatus,
  type Traffic,
} from './RemotePort.ts'
import {
  type AdmissionError,
  OpStalled,
  type OpTransportError,
  RelayDown,
} from './errors.ts'

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

const count = (
  status: SubscriptionRef.SubscriptionRef<RemoteStatus>,
  field: keyof Traffic,
  bytes: number,
) =>
  SubscriptionRef.update(status, current => ({
    ...current,
    traffic: { ...current.traffic, [field]: current.traffic[field] + bytes },
  }))

type OpQueue = Queue.Queue<OpEvent, OpTransportError | Cause.Done>

/**
 * Ops waiting for the page, by id. Ids are unique per connection; an event
 * for an id nobody waits on (an op given up on, then resent) is dropped.
 */
const pendingOps = Effect.gen(function* () {
  const nextId = yield* Ref.make(1)
  const waiting = yield* Ref.make<ReadonlyMap<RequestId, OpQueue>>(new Map())
  const find = (id: RequestId) =>
    Ref.get(waiting).pipe(Effect.map(all => Option.fromNullishOr(all.get(id))))
  return {
    open: Effect.gen(function* () {
      const id = RequestId.make(yield* Ref.getAndUpdate(nextId, n => n + 1))
      const queue: OpQueue = yield* Queue.unbounded<
        OpEvent,
        OpTransportError | Cause.Done
      >()
      yield* Ref.update(waiting, all => new Map(all).set(id, queue))
      return { id, queue }
    }),
    forget: (id: RequestId) =>
      Ref.update(waiting, all => {
        const rest = new Map(all)
        rest.delete(id)
        return rest
      }),
    deliver: (id: RequestId, event: OpEvent) =>
      find(id).pipe(
        Effect.flatMap(
          Option.match({
            onNone: () => Effect.void,
            onSome: queue =>
              Queue.offer(queue, event).pipe(
                Effect.andThen(
                  event._tag === 'Result' ? Queue.end(queue) : Effect.void,
                ),
              ),
          }),
        ),
      ),
    failAll: Ref.getAndSet(waiting, new Map()).pipe(
      Effect.flatMap(all =>
        Effect.forEach(all.values(), queue =>
          Queue.fail(queue, new RelayDown()),
        ),
      ),
    ),
  }
})

export const layer: Layer.Layer<Remote, never, RelayClient> = Layer.effect(
  Remote,
  Effect.gen(function* () {
    const relay = yield* RelayClient

    const attach = Effect.fn('Remote.attach')(function* (code: Code) {
      const link = yield* relay.connect(code)
      const status = yield* SubscriptionRef.make(initialStatus)
      const output = yield* PubSub.unbounded<Uint8Array>()
      const pending = yield* pendingOps

      const onControl = (message: ToAgent) =>
        SubscriptionRef.update(status, current =>
          applyMessage(current, message),
        ).pipe(
          Effect.andThen(
            Match.value(message).pipe(
              Match.tag('OpProgress', ({ id, done, total }) =>
                pending.deliver(id, { _tag: 'Progress', done, total }),
              ),
              Match.tag('OpResult', ({ id, outcome }) =>
                pending.deliver(id, { _tag: 'Result', outcome }),
              ),
              Match.orElse(() => Effect.void),
            ),
          ),
        )

      const setLink = (next: Link) =>
        SubscriptionRef.update(status, current => ({ ...current, link: next }))

      const onInbound = (event: Inbound) =>
        Match.valueTags(event, {
          Console: ({ bytes }) =>
            PubSub.publish(output, bytes).pipe(
              Effect.andThen(count(status, 'fromDevice', bytes.length)),
            ),
          OpData: ({ id, offset, bytes }) =>
            pending.deliver(id, { _tag: 'Data', offset, bytes }),
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

      const write = (bytes: Uint8Array) =>
        Effect.gen(function* () {
          const open = isPortOpen(yield* SubscriptionRef.get(status))
          const sent = open && (yield* link.send(bytes))
          yield* count(status, sent ? 'toDevice' : 'dropped', bytes.length)
          return sent
        })

      const op = (request: Op, upload?: Uint8Array) =>
        Stream.unwrap(
          Effect.gen(function* () {
            if ((yield* SubscriptionRef.get(status)).link._tag !== 'Joined') {
              return yield* new RelayDown()
            }
            const { id, queue } = yield* pending.open
            if (!(yield* link.send({ _tag: 'Op', id, op: request }))) {
              yield* pending.forget(id)
              return yield* new RelayDown()
            }
            for (const frame of opDataFrames(id, upload ?? new Uint8Array())) {
              yield* link.send(frame)
            }
            return Stream.fromQueue(queue).pipe(
              Stream.timeoutOrElse({
                duration: OP_STALL_MS,
                orElse: () => Stream.fail(new OpStalled({ ms: OP_STALL_MS })),
              }),
              Stream.ensuring(pending.forget(id)),
            )
          }),
        )

      return {
        device: firstMatch(status, current => current.device),
        output: Stream.fromPubSub(output),
        listen: PubSub.subscribe(output).pipe(
          Effect.map(Stream.fromSubscription),
        ),
        write,
        status,
        op,
      } satisfies RemotePort
    })

    return Remote.of({ attach })
  }),
)
