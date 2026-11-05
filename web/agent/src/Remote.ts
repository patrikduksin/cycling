import * as Context from 'effect/Context'
import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as PubSub from 'effect/PubSub'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'

import type { Code, ToAgent } from '@cycling/protocol'

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
import type { AdmissionError } from './errors.ts'

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

export const layer: Layer.Layer<Remote, never, RelayClient> = Layer.effect(
  Remote,
  Effect.gen(function* () {
    const relay = yield* RelayClient

    const attach = Effect.fn('Remote.attach')(function* (code: Code) {
      const link = yield* relay.connect(code)
      const status = yield* SubscriptionRef.make(initialStatus)
      const output = yield* PubSub.unbounded<Uint8Array>()

      const onControl = (message: ToAgent) =>
        SubscriptionRef.update(status, current =>
          applyMessage(current, message),
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
          Down: () => setLink({ _tag: 'Redialing' }),
          Stopped: ({ ending }) => setLink({ _tag: 'Over', ending }),
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

      return {
        device: firstMatch(status, current => current.device),
        output: Stream.fromPubSub(output),
        write,
        status,
      } satisfies RemotePort
    })

    return Remote.of({ attach })
  }),
)
