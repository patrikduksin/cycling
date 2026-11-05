import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import type { Update } from 'foldkit'
import { modifyFields } from 'foldkit/struct'

import {
  type Op,
  type OpOutcome,
  type PortFailureReason,
  type RequestId,
  type ToBrowser,
  backoffMs,
  closeMeaning,
} from '@cycling/protocol'

import {
  ClearSession,
  MintSession,
  SaveSession,
  SayBye,
  SendControl,
  SendEntered,
  WaitForAgent,
  WaitRelayRetry,
} from './command.ts'
import type { HandleServices } from './handles.ts'
import {
  type Step,
  failQueued,
  onAgentLeft,
  onLostFlasher,
  onOp,
  runningWrite,
  startQueuedEntry,
} from './holder.ts'
import type { Chooser } from './links.ts'
import {
  type PortStep,
  announcement,
  failed,
  opened,
  reacquire,
  replaced,
  storedOf,
} from './live.ts'
import type { Message } from './message.ts'
import {
  type Ending,
  type Live,
  type Model,
  Page,
  type Port,
  Relay,
  type Session,
  Source,
} from './model.ts'
import { emptyMonitor } from './monitor.ts'

type Services = Chooser | HandleServices
export type UpdateReturn = Update.Return<Model, Message, Services>

export const ignore = (model: Model): UpdateReturn => ({ model })

export const withPage = (model: Model, page: Page): Model =>
  modifyFields(model, { page: () => page })

type PageOf<T extends Page['_tag']> = Extract<Page, { readonly _tag: T }>

const isPage =
  <T extends Page['_tag']>(tag: T) =>
  (page: Page): page is PageOf<T> =>
    page._tag === tag

export const whenPage = <T extends Page['_tag']>(
  model: Model,
  tag: T,
  handle: (page: PageOf<T>) => UpdateReturn,
): UpdateReturn =>
  isPage(tag)(model.page) ? handle(model.page) : ignore(model)

export const withSerial = (
  model: Model,
  live: Live,
  handle: (port: Port) => UpdateReturn,
): UpdateReturn =>
  Source.matchOrElse(live.source, { Serial: ({ port }) => handle(port) }, () =>
    ignore(model),
  )

export const withLive = (
  model: Model,
  live: Live,
  fields: Partial<Omit<Live, '_tag'>>,
): Model => withPage(model, { ...live, ...fields })

export const ended = (model: Model, reason: Ending): UpdateReturn => ({
  model: withPage(model, Page.Ended({ reason })),
  commands: [ClearSession()],
})

const savePort = (live: Live, port: Port) =>
  SaveSession({ stored: storedOf(live.session, port, live.writesAllowed) })

/**
 * Applies a step of the holder table: the port's next value, its OpResults
 * and PortChanged in order, an Enter to answer from the session, and the wait
 * for an agent that left. `save` writes the new epoch as well.
 */
export const holdStep = (
  model: Model,
  live: Live,
  next: Step,
  save = false,
): UpdateReturn => {
  const nextModel = withLive(model, live, {
    source: Source.Serial({ port: next.port }),
  })
  const page = nextModel.page
  const commands = [
    ...(next.messages.length === 0
      ? []
      : [SendControl({ messages: next.messages })]),
    ...Option.toArray(Option.map(next.entered, id => SendEntered({ id }))),
    ...Option.toArray(Option.map(next.awaitAgent, away => WaitForAgent(away))),
    ...(save && isPage('Live')(page) ? [savePort(page, next.port)] : []),
  ]
  return commands.length === 0
    ? { model: nextModel }
    : { model: nextModel, commands }
}

/** A holder transition on the serial port, or nothing when it does not apply. */
export const withHold = (
  model: Model,
  live: Live,
  transition: (port: Port) => Option.Option<Step>,
): UpdateReturn =>
  withSerial(model, live, port =>
    Option.match(transition(port), {
      onNone: () => ignore(model),
      onSome: next => holdStep(model, live, next),
    }),
  )

/** `save` writes the port's new baud or epoch to sessionStorage as well. */
const portStep = (
  model: Model,
  live: Live,
  step: PortStep,
  save: boolean,
): UpdateReturn => ({
  model: withLive(model, live, { source: Source.Serial({ port: step.port }) }),
  commands: [
    SendControl({ messages: step.messages }),
    ...(save ? [savePort(live, step.port)] : []),
  ],
})

export const startLive = (
  model: Model,
  fields: { readonly session: Session; readonly source: Source },
): Model =>
  withPage(
    model,
    Page.Live({
      ...fields,
      relay: Relay.Dialing({ attempt: 0, failures: 0 }),
      monitor: emptyMonitor,
      copied: false,
      writesAllowed: false,
      confirmingStop: false,
    }),
  )

const backoff = (
  model: Model,
  live: Live,
  relay: { readonly attempt: number; readonly failures: number },
): UpdateReturn => ({
  model: withLive(model, live, { relay: Relay.Backoff(relay) }),
  commands: [
    WaitRelayRetry({
      attempt: relay.attempt,
      delayMs: backoffMs(relay.failures),
    }),
  ],
})

export const onJoinedRelay = (model: Model, live: Live): UpdateReturn =>
  Relay.matchOrElse(
    live.relay,
    {
      Dialing: ({ attempt }) => ({
        model: withLive(model, live, {
          relay: Relay.Joined({ attempt, agentPresent: false }),
        }),
        commands: [SendControl({ messages: announcement(live.source) })],
      }),
    },
    () => ignore(model),
  )

export const onFailedDial = (model: Model, live: Live): UpdateReturn =>
  Relay.matchOrElse(
    live.relay,
    {
      Dialing: ({ attempt, failures }) =>
        backoff(model, live, { attempt, failures: failures + 1 }),
    },
    () => ignore(model),
  )

export const onRetryDue = (
  model: Model,
  live: Live,
  due: number,
): UpdateReturn =>
  Relay.matchOrElse(
    live.relay,
    {
      Backoff: ({ attempt, failures }) =>
        attempt === due
          ? {
              model: withLive(model, live, {
                relay: Relay.Dialing({ attempt: attempt + 1, failures }),
              }),
            }
          : ignore(model),
    },
    () => ignore(model),
  )

export const onClosedRelay = (
  model: Model,
  live: Live,
  closed: { readonly attempt: number; readonly code: number },
): UpdateReturn =>
  Relay.matchOrElse(
    live.relay,
    {
      Joined: ({ attempt }) =>
        attempt !== closed.attempt
          ? ignore(model)
          : Match.value(closeMeaning(closed.code)).pipe(
              Match.withReturnType<UpdateReturn>(),
              Match.tagsExhaustive({
                Transient: () => backoff(model, live, { attempt, failures: 1 }),
                Remint: () => ({
                  model: withLive(model, live, {
                    relay: Relay.Backoff({ attempt, failures: 0 }),
                  }),
                  commands: [MintSession()],
                }),
                Ended: ({ reason }) => ended(model, reason),
                Replaced: () => ended(model, 'replaced'),
                Refused: ({ why }) => ended(model, why),
              }),
            ),
      Leaving: () => ended(model, 'visitorDisconnected'),
    },
    () => ignore(model),
  )

export const onMintedSession = (
  model: Model,
  live: Live,
  session: Session,
): UpdateReturn =>
  Relay.matchOrElse(
    live.relay,
    {
      Backoff: ({ attempt }) => {
        const minted = {
          ...live,
          session,
          relay: Relay.Dialing({ attempt: attempt + 1, failures: 0 }),
          copied: false,
          writesAllowed: false,
        }
        return {
          model: withPage(model, minted),
          commands: Source.match(live.source, {
            Serial: ({ port }) => [savePort(minted, port)],
            Usb: () => [],
          }),
        }
      },
    },
    () => ignore(model),
  )

/** An agent that leaves during a hold starts the wait for it to come back. */
const onPeerPresence = (
  model: Model,
  live: Live,
  present: boolean,
): UpdateReturn =>
  Relay.matchOrElse(
    live.relay,
    {
      Joined: ({ attempt }) => {
        const joined = {
          ...live,
          relay: Relay.Joined({ attempt, agentPresent: present }),
        }
        const next = withPage(model, joined)
        return present ? { model: next } : withHold(next, joined, onAgentLeft)
      },
    },
    () => ignore(model),
  )

/** A described USB device holds no port: only Release has an answer. */
const answerWithoutPort = (op: Op): OpOutcome =>
  op._tag === 'Release'
    ? { _tag: 'Released' }
    : { _tag: 'Failed', reason: 'notEntered', detail: '' }

const onOpMessage = (
  model: Model,
  live: Live,
  id: RequestId,
  op: Op,
): UpdateReturn =>
  Source.match(live.source, {
    Serial: ({ port }) => holdStep(model, live, onOp(port, id, op)),
    Usb: () => ({
      model,
      commands: [
        SendControl({
          messages: [{ _tag: 'OpResult', id, outcome: answerWithoutPort(op) }],
        }),
      ],
    }),
  })

export const onRelayMessage = (
  model: Model,
  live: Live,
  message: ToBrowser,
): UpdateReturn =>
  Match.value(message).pipe(
    Match.withReturnType<UpdateReturn>(),
    Match.tagsExhaustive({
      Welcome: ({ snapshot }) =>
        onPeerPresence(model, live, snapshot.agentPresent),
      PeerPresence: ({ role, present }) =>
        role === 'agent' ? onPeerPresence(model, live, present) : ignore(model),
      Op: ({ id, op }) => onOpMessage(model, live, id, op),
    }),
  )

export const onDisconnect = (model: Model, live: Live): UpdateReturn =>
  Relay.match<UpdateReturn>(live.relay, {
    Joined: ({ attempt }) => ({
      model: withLive(model, live, { relay: Relay.Leaving({ attempt }) }),
      commands: [SayBye()],
    }),
    Leaving: () => ignore(model),
    Dialing: () => ended(model, 'visitorDisconnected'),
    Backoff: () => ended(model, 'visitorDisconnected'),
  })

const isStarting = (port: Port): boolean =>
  port.state._tag === 'Opening' || port.state._tag === 'Reacquiring'

/**
 * During a write, Disconnect first asks; a second answer
 * (`ClickedStopAndDisconnect`) stops it.
 */
export const onClickedDisconnect = (model: Model, live: Live): UpdateReturn =>
  Source.match(live.source, {
    Serial: ({ port }) =>
      Option.isSome(runningWrite(port, live.writesAllowed)) &&
      !live.confirmingStop
        ? { model: withLive(model, live, { confirmingStop: true }) }
        : onDisconnect(model, live),
    Usb: () => onDisconnect(model, live),
  })

/** An Enter that waited for the console starts as soon as it is open. */
export const onOpenedPort = (model: Model, live: Live): UpdateReturn =>
  withSerial(model, live, port => {
    if (!isStarting(port)) {
      return ignore(model)
    }
    const step = opened(port)
    return holdStep(
      model,
      live,
      {
        port: startQueuedEntry(step.port),
        messages: step.messages,
        entered: Option.none(),
        awaitAgent: Option.none(),
      },
      true,
    )
  })

/**
 * `attempt` tells a stream that ended on its own from one the runtime
 * already replaced. A lost flasher ends its hold.
 */
export const onLostPort = (
  model: Model,
  live: Live,
  attempt: number,
): UpdateReturn =>
  withSerial(model, live, port =>
    port.attempt === attempt && port.state._tag === 'Open'
      ? portStep(model, live, reacquire(port), false)
      : withHold(model, live, held => onLostFlasher(held, attempt)),
  )

export const onFailedPort = (
  model: Model,
  live: Live,
  reason: PortFailureReason,
): UpdateReturn =>
  withSerial(model, live, port => {
    if (!isStarting(port)) {
      return ignore(model)
    }
    const step = failed(port, reason)
    const refused = failQueued(step.port)
    return holdStep(model, live, {
      ...refused,
      messages: [...step.messages, ...refused.messages],
    })
  })

export const onChoseSerialAgain = (
  model: Model,
  live: Live,
  identity: Port['identity'],
): UpdateReturn =>
  withSerial(model, live, port =>
    port.state._tag === 'Failed'
      ? portStep(model, live, replaced(port, identity), true)
      : ignore(model),
  )
