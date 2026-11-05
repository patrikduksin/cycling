import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Predicate from 'effect/Predicate'
import * as Schema from 'effect/Schema'
import * as Stream from 'effect/Stream'
import { ManagedResource, Subscription } from 'foldkit'

import { Baud, Code, OwnerKey, SerialIdentity } from '@cycling/protocol'

import { deviceToRelay, relayToDevice } from './dataplane.ts'
import { type HandleServices, RelayHandle, SerialHandle } from './handles.ts'
import type { Adapters } from './links.ts'
import { Message } from './message.ts'
import { type Model, Relay, livePage, livePort } from './model.ts'
import { REACQUIRE } from './policy.ts'

const SerialRequirement = Schema.Struct({
  identity: SerialIdentity,
  baud: Baud,
  attempt: Schema.Int,
})

const RelayRequirement = Schema.Struct({
  code: Code,
  ownerKey: OwnerKey,
  attempt: Schema.Int,
})

/**
 * The port is held for as long as the page is live on a serial device that has
 * not failed. Ending the session turns this into None, which is the whole
 * disconnect: the runtime closes the port.
 */
export const serialRequirement = (
  model: Model,
): Option.Option<typeof SerialRequirement.Type> =>
  livePort(model).pipe(
    Option.filter(port => port.state._tag !== 'Failed'),
    Option.map(({ identity, baud, attempt }) => ({ identity, baud, attempt })),
  )

/** The socket is held while dialing, joined or leaving; backoff and the end of the session release it. */
export const relayRequirement = (
  model: Model,
): Option.Option<typeof RelayRequirement.Type> =>
  livePage(model).pipe(
    Option.filter(({ relay }) => !Relay.guards.Backoff(relay)),
    Option.map(({ session, relay }) => ({
      code: session.code,
      ownerKey: session.ownerKey,
      attempt: relay.attempt,
    })),
  )

/** Present only while the port is Open: the attempt that opened it, so a stale stream can be told apart. */
export const openPortAttempt = (model: Model): Option.Option<number> =>
  livePort(model).pipe(
    Option.filter(port => port.state._tag === 'Open'),
    Option.map(port => port.attempt),
  )

/** Present while the socket is joined or leaving, so control keeps flowing while the port is gone. */
export const joinedRelayAttempt = (model: Model): Option.Option<number> =>
  livePage(model).pipe(
    Option.map(live => live.relay),
    Option.filter(
      relay => Relay.guards.Joined(relay) || Relay.guards.Leaving(relay),
    ),
    Option.map(relay => relay.attempt),
  )

/**
 * Both handles follow the Model: bumping `attempt` re-keys the requirement,
 * which is how a USB re-enumeration and a relay redial both become "acquire
 * again".
 */
export const managedResources = (adapters: Adapters) =>
  ManagedResource.make<Model, Message>()(entry => ({
    serial: entry(Schema.Option(SerialRequirement), {
      resource: SerialHandle,
      modelToMaybeRequirements: serialRequirement,
      acquire: ({ identity, baud, attempt }) =>
        adapters.serial.acquire({
          identity,
          baud,
          waitMs: attempt === 0 ? 0 : REACQUIRE.timeoutMs,
        }),
      release: () => Effect.void,
      onAcquired: () => Message.OpenedPort(),
      onReleased: () => Message.ReleasedSerial(),
      onAcquireError: error =>
        Message.FailedPort({
          reason: Predicate.isTagged(error, 'ReacquireTimeout')
            ? 'reacquireTimeout'
            : 'openRefused',
        }),
    }),
    relay: entry(Schema.Option(RelayRequirement), {
      resource: RelayHandle,
      modelToMaybeRequirements: relayRequirement,
      acquire: ({ code, ownerKey }) =>
        Effect.fromOption(adapters.relay).pipe(
          Effect.flatMap(relay => relay.dial({ code, ownerKey })),
        ),
      release: () => Effect.void,
      onAcquired: () => Message.JoinedRelay(),
      onReleased: () => Message.ReleasedRelay(),
      onAcquireError: () => Message.FailedDialRelay(),
    }),
  }))

/**
 * Neither Subscription routes bytes through `update`; they hand it one
 * Traffic Message per direction per window.
 *
 *   deviceToRelay  port Open      serial input, coalesced, to the relay
 *   relayToDevice  relay joined   relay frames: bytes to the port, control to update
 */
export const subscriptions = Subscription.make<
  Model,
  Message,
  HandleServices
>()(entry => ({
  deviceToRelay: entry(
    { attempt: Schema.Option(Schema.Int) },
    {
      modelToDependencies: model => ({ attempt: openPortAttempt(model) }),
      dependenciesToStream: ({ attempt }) =>
        Option.match(attempt, {
          onNone: () => Stream.empty,
          onSome: attempt =>
            Stream.unwrap(
              SerialHandle.get.pipe(
                Effect.map(serial =>
                  deviceToRelay({
                    input: serial.input,
                    attempt,
                    send: bytes =>
                      RelayHandle.get.pipe(
                        Effect.flatMap(relay => relay.send(bytes)),
                        Effect.orElseSucceed(() => false),
                      ),
                  }),
                ),
                Effect.orElseSucceed(() => Stream.empty),
              ),
            ),
        }),
    },
  ),
  relayToDevice: entry(
    { attempt: Schema.Option(Schema.Int) },
    {
      modelToDependencies: model => ({ attempt: joinedRelayAttempt(model) }),
      dependenciesToStream: ({ attempt }) =>
        Option.match(attempt, {
          onNone: () => Stream.empty,
          onSome: attempt =>
            Stream.unwrap(
              RelayHandle.get.pipe(
                Effect.map(relay =>
                  relayToDevice({
                    events: relay.events,
                    attempt,
                    write: bytes =>
                      SerialHandle.get.pipe(
                        Effect.flatMap(serial => serial.write(bytes)),
                        Effect.as(true),
                        Effect.orElseSucceed(() => false),
                      ),
                  }),
                ),
                Effect.orElseSucceed(() => Stream.empty),
              ),
            ),
        }),
    },
  ),
}))
