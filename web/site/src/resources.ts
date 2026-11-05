import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as Predicate from 'effect/Predicate'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import { ManagedResource, Subscription } from 'foldkit'

import type { Session } from '@cycling/flasher'
import {
  Baud,
  type Binary,
  type BrowserMsg,
  Code,
  OP_PROGRESS_MS,
  OwnerKey,
  RequestId,
  SerialIdentity,
} from '@cycling/protocol'

import { deviceToRelay, relayToDevice } from './dataplane.ts'
import { makeInbox } from './flasher/inbox.ts'
import {
  PARTITION_TABLE,
  bootSlotOf,
  otadataOf,
  parsePartitions,
} from './flasher/partitions.ts'
import { runOp } from './flasher/run.ts'
import { type HandleServices, PortHandle, RelayHandle } from './handles.ts'
import { headOf, isHold } from './holder.ts'
import {
  type Adapters,
  EntryFailed,
  type PortLink,
  type ReacquireTimeout,
  type SerialOpenRefused,
} from './links.ts'
import { Message } from './message.ts'
import {
  Entry,
  type Layout,
  type Model,
  Relay,
  RunnableOp,
  livePage,
  livePort,
} from './model.ts'
import { REACQUIRE } from './policy.ts'

/**
 * The console needs the baud; the flasher opens the port itself and needs
 * the entry. A change of tag, attempt or entry re-keys the one port
 * resource, so the old holder closes before the new one opens.
 */
const PortRequirement = Schema.Union([
  Schema.TaggedStruct('Console', {
    identity: SerialIdentity,
    baud: Baud,
    attempt: Schema.Int,
  }),
  Schema.TaggedStruct('Flasher', {
    identity: SerialIdentity,
    attempt: Schema.Int,
    entry: Entry,
  }),
])
type PortRequirement = typeof PortRequirement.Type

const RelayRequirement = Schema.Struct({
  code: Code,
  ownerKey: OwnerKey,
  attempt: Schema.Int,
})

/**
 * The port is held for as long as the page is live on a serial device that has
 * not failed. Ending the session turns this into None, which is the whole
 * disconnect: the runtime closes the port, or the flasher session.
 */
export const portRequirement = (model: Model): Option.Option<PortRequirement> =>
  livePort(model).pipe(
    Option.filter(port => port.state._tag !== 'Failed'),
    Option.map(({ identity, baud, attempt, state }) =>
      isHold(state)
        ? { _tag: 'Flasher', identity, attempt, entry: state.entry }
        : { _tag: 'Console', identity, baud, attempt },
    ),
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

/** Present only while the console has the port Open: the attempt that opened it, so a stale stream can be told apart. */
export const openPortAttempt = (model: Model): Option.Option<number> =>
  livePort(model).pipe(
    Option.filter(port => port.state._tag === 'Open'),
    Option.map(port => port.attempt),
  )

/** Present while the flasher holds the port, for the stream that reports it lost. */
export const flashingAttempt = (model: Model): Option.Option<number> =>
  livePort(model).pipe(
    Option.filter(port => port.state._tag === 'Flashing'),
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

const RunningOp = Schema.Struct({
  attempt: Schema.Int,
  id: RequestId,
  op: RunnableOp,
})

/** The head of the queue, while the flasher holds the port and nothing waits for the visitor. */
export const runningOp = (model: Model): Option.Option<typeof RunningOp.Type> =>
  livePage(model).pipe(
    Option.flatMap(live =>
      livePort(model).pipe(
        Option.flatMap(port =>
          headOf(port).pipe(
            Option.filter(
              head => head.op._tag !== 'Write' || live.writesAllowed,
            ),
            Option.map(({ id, op }) => ({ attempt: port.attempt, id, op })),
          ),
        ),
      ),
    ),
  )

const WaitingOp = Schema.Struct({ id: RequestId, total: Schema.Int })

/**
 * The head of the queue while it waits: for the flasher to enter, for the
 * console to come back, or for the visitor's Allow. The agent still hears
 * it is alive.
 */
export const waitingOp = (model: Model): Option.Option<typeof WaitingOp.Type> =>
  Option.isSome(runningOp(model))
    ? Option.none()
    : livePort(model).pipe(
        Option.flatMap(port => Option.fromNullishOr(port.ops[0])),
        Option.map(({ id, total }) => ({ id, total })),
      )

/** Collects one region read from the session. */
const readAll = (session: Session, region: { address: number; size: number }) =>
  session.read(region).pipe(
    Stream.runFold(
      () => new Uint8Array(region.size),
      (bytes, event) => {
        if (event._tag === 'Piece') {
          bytes.set(event.bytes, event.offset)
        }
        return bytes
      },
    ),
  )

/**
 * The device's own partition table and otadata, to name regions for the
 * visitor. A device without them, or a read that fails, is shown by address.
 */
const readLayout = (session: Session): Effect.Effect<Option.Option<Layout>> =>
  readAll(session, PARTITION_TABLE).pipe(
    Effect.map(parsePartitions),
    Effect.flatMap(partitions =>
      Option.match(otadataOf(partitions), {
        onNone: () => Effect.succeed(Option.none<string>()),
        onSome: region =>
          readAll(session, region).pipe(
            Effect.map(otadata => bootSlotOf(partitions, otadata)),
          ),
      }).pipe(
        Effect.map(bootSlot =>
          partitions.length === 0
            ? Option.none()
            : Option.some({ partitions, bootSlot }),
        ),
      ),
    ),
    Effect.orElseSucceed(() => Option.none()),
  )

/**
 * Both handles follow the Model: bumping `attempt` re-keys the requirement,
 * which is how a USB re-enumeration and a relay redial both become "acquire
 * again". Foldkit's re-key runs the old acquire's release and closes its
 * scope before the new acquire starts (a switchMap over the requirement), so
 * the console has closed the SerialPort before the flasher opens it, and the
 * flasher's finalizer has reset the chip and closed it before the console
 * looks for it again.
 */
export const managedResources = (adapters: Adapters) => {
  const acquirePort = (
    requirement: PortRequirement,
  ): Effect.Effect<
    PortLink,
    EntryFailed | SerialOpenRefused | ReacquireTimeout,
    Scope.Scope
  > =>
    Match.value(requirement).pipe(
      Match.withReturnType<
        Effect.Effect<
          PortLink,
          EntryFailed | SerialOpenRefused | ReacquireTimeout,
          Scope.Scope
        >
      >(),
      Match.tagsExhaustive({
        Console: ({ identity, baud, attempt }) =>
          adapters.serial
            .acquire({
              identity,
              baud,
              waitMs: attempt === 0 ? 0 : REACQUIRE.timeoutMs,
            })
            .pipe(Effect.map((link): PortLink => ({ _tag: 'Console', link }))),
        Flasher: ({ identity, entry }) =>
          Effect.gen(function* () {
            const port = yield* adapters.serial
              .granted({ identity, waitMs: REACQUIRE.timeoutMs })
              .pipe(
                Effect.mapError(
                  () =>
                    new EntryFailed({
                      reason: 'entryFailed',
                      detail: 'The device did not come back.',
                    }),
                ),
              )
            const session = yield* adapters.flasher.open(port, { entry }).pipe(
              Effect.mapError(
                ({ reason, detail }) =>
                  new EntryFailed({
                    reason:
                      reason === 'portLost' || reason === 'unsupportedChip'
                        ? reason
                        : 'entryFailed',
                    detail,
                  }),
              ),
            )
            const inbox = yield* makeInbox(adapters.flasher.md5Hex)
            const layout = yield* readLayout(session)
            return {
              _tag: 'Flasher',
              session,
              inbox,
              layout,
            } satisfies PortLink
          }),
      }),
    )

  return ManagedResource.make<Model, Message>()(entry => ({
    port: entry(Schema.Option(PortRequirement), {
      resource: PortHandle,
      modelToMaybeRequirements: portRequirement,
      acquire: acquirePort,
      release: () => Effect.void,
      onAcquired: (link: PortLink) =>
        link._tag === 'Console'
          ? Message.OpenedPort()
          : Message.EnteredFlasher({ layout: link.layout }),
      onReleased: () => Message.ReleasedSerial(),
      onAcquireError: error =>
        error instanceof EntryFailed
          ? Message.FailedEntry({ reason: error.reason, detail: error.detail })
          : Message.FailedPort({
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
}

const sendToRelay = (item: Binary | BrowserMsg) =>
  RelayHandle.get.pipe(
    Effect.flatMap(relay => relay.send(item)),
    Effect.orElseSucceed(() => false),
  )

/** The held port, if `tag` holds it; any other holder is nothing to this stream. */
const portAs = <T extends PortLink['_tag']>(tag: T) =>
  PortHandle.get.pipe(
    Effect.option,
    Effect.map(
      Option.filter(
        (link): link is Extract<PortLink, { readonly _tag: T }> =>
          link._tag === tag,
      ),
    ),
  )

const whenHeld = <T extends PortLink['_tag'], A>(
  tag: T,
  stream: (
    link: Extract<PortLink, { readonly _tag: T }>,
  ) => Stream.Stream<A, never, HandleServices>,
): Stream.Stream<A, never, HandleServices> =>
  Stream.unwrap(
    portAs(tag).pipe(
      Effect.map(Option.match({ onNone: () => Stream.empty, onSome: stream })),
    ),
  )

/**
 * No Subscription routes bytes through `update`; they hand it one Traffic
 * Message per direction per window, and redacted progress for ops.
 *
 *   deviceToRelay  console Open     serial input, coalesced, to the relay
 *   relayToDevice  relay joined     console frames to the port, write payloads to
 *                                   the flasher's inbox, control to update
 *   opRunner       Flashing, head   the head op against the session
 *   opWaiting      a head waits     OpProgress for it once a second
 *   flasherLost    Flashing         LostPort when the device goes away
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
            whenHeld('Console', ({ link }) =>
              deviceToRelay({
                input: link.input,
                attempt,
                send: bytes => sendToRelay({ _tag: 'Console', bytes }),
              }),
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
                      portAs('Console').pipe(
                        Effect.flatMap(
                          Option.match({
                            onNone: () => Effect.succeed(false),
                            onSome: ({ link }) =>
                              link.write(bytes).pipe(
                                Effect.as(true),
                                Effect.orElseSucceed(() => false),
                              ),
                          }),
                        ),
                      ),
                    deliver: frame =>
                      portAs('Flasher').pipe(
                        Effect.flatMap(
                          Option.match({
                            onNone: () => Effect.void,
                            onSome: ({ inbox }) => inbox.offer(frame),
                          }),
                        ),
                      ),
                  }),
                ),
                Effect.orElseSucceed(() => Stream.empty),
              ),
            ),
        }),
    },
  ),
  opRunner: entry(
    { running: Schema.Option(RunningOp) },
    {
      modelToDependencies: model => ({ running: runningOp(model) }),
      dependenciesToStream: ({ running }) =>
        Option.match(running, {
          onNone: () => Stream.empty,
          onSome: ({ attempt, id, op }) =>
            whenHeld('Flasher', ({ session, inbox }) =>
              runOp({ attempt, id, op, session, inbox, send: sendToRelay }),
            ),
        }),
    },
  ),
  opWaiting: entry(
    { waiting: Schema.Option(WaitingOp) },
    {
      modelToDependencies: model => ({ waiting: waitingOp(model) }),
      dependenciesToStream: ({ waiting }) =>
        Option.match(waiting, {
          onNone: () => Stream.empty,
          onSome: ({ id, total }) =>
            Stream.tick(Duration.millis(OP_PROGRESS_MS)).pipe(
              Stream.mapEffect(() =>
                sendToRelay({ _tag: 'OpProgress', id, done: 0, total }),
              ),
              Stream.drain,
            ),
        }),
    },
  ),
  flasherLost: entry(
    { attempt: Schema.Option(Schema.Int) },
    {
      modelToDependencies: model => ({ attempt: flashingAttempt(model) }),
      dependenciesToStream: ({ attempt }) =>
        Option.match(attempt, {
          onNone: () => Stream.empty,
          onSome: attempt =>
            whenHeld('Flasher', ({ session }) =>
              Stream.fromEffect(
                session.lost.pipe(Effect.as(Message.LostPort({ attempt }))),
              ),
            ),
        }),
    },
  ),
}))
