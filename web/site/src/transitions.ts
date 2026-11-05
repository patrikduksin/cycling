import * as Match from 'effect/Match'
import type { Update } from 'foldkit'
import { modifyFields } from 'foldkit/struct'

import {
  type Op,
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
  WaitRelayRetry,
} from './command.ts'
import type { HandleServices } from './handles.ts'
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

const savePort = (session: Session, port: Port) =>
  SaveSession({ stored: storedOf(session, port) })

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
    ...(save ? [savePort(live.session, step.port)] : []),
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
      Backoff: ({ attempt }) => ({
        model: withLive(model, live, {
          session,
          relay: Relay.Dialing({ attempt: attempt + 1, failures: 0 }),
          copied: false,
        }),
        commands: Source.match(live.source, {
          Serial: ({ port }) => [savePort(session, port)],
          Usb: () => [],
        }),
      }),
    },
    () => ignore(model),
  )

const onPeerPresence = (
  model: Model,
  live: Live,
  present: boolean,
): UpdateReturn =>
  Relay.matchOrElse(
    live.relay,
    {
      Joined: ({ attempt }) => ({
        model: withLive(model, live, {
          relay: Relay.Joined({ attempt, agentPresent: present }),
        }),
      }),
    },
    () => ignore(model),
  )

/**
 * Every op is refused until the page's flasher can hold the port; a Release
 * is answered as the protocol promises, whatever is held.
 */
const answerOp = (model: Model, id: RequestId, op: Op): UpdateReturn => ({
  model,
  commands: [
    SendControl({
      messages: [
        {
          _tag: 'OpResult',
          id,
          outcome:
            op._tag === 'Release'
              ? { _tag: 'Released' }
              : { _tag: 'Failed', reason: 'notEntered', detail: '' },
        },
      ],
    }),
  ],
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
      Op: ({ id, op }) => answerOp(model, id, op),
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

export const onOpenedPort = (model: Model, live: Live): UpdateReturn =>
  withSerial(model, live, port =>
    isStarting(port)
      ? portStep(model, live, opened(port), true)
      : ignore(model),
  )

/** `attempt` tells a stream that ended on its own from one the runtime already replaced. */
export const onLostPort = (
  model: Model,
  live: Live,
  attempt: number,
): UpdateReturn =>
  withSerial(model, live, port =>
    port.attempt === attempt && port.state._tag === 'Open'
      ? portStep(model, live, reacquire(port), false)
      : ignore(model),
  )

export const onFailedPort = (
  model: Model,
  live: Live,
  reason: PortFailureReason,
): UpdateReturn =>
  withSerial(model, live, port =>
    isStarting(port)
      ? portStep(model, live, failed(port, reason), false)
      : ignore(model),
  )

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
