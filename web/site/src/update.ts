import * as Option from 'effect/Option'

import { formatCode } from '@cycling/protocol'

import {
  ChooseSerial,
  ChooseSerialAgain,
  ChooseUsb,
  CopyCode,
  SaveSession,
  SendControl,
} from './command.ts'
import { newPort, recoveredPort, storedOf } from './live.ts'
import { Message } from './message.ts'
import { type Flags, type Model, Page, Relay, Source } from './model.ts'
import { emptyMonitor, record } from './monitor.ts'
import { detectMissing } from './policy.ts'
import {
  type UpdateReturn,
  ended,
  ignore,
  onChoseSerialAgain,
  onClosedRelay,
  onDisconnect,
  onFailedDial,
  onFailedPort,
  onJoinedRelay,
  onLostPort,
  onMintedSession,
  onOpenedPort,
  onRelayMessage,
  onRetryDue,
  startLive,
  whenPage,
  withLive,
  withPage,
  withSerial,
} from './transitions.ts'

export const update = (model: Model, message: Message): UpdateReturn =>
  Message.match<UpdateReturn>(message, {
    ClickedConnectDevice: () =>
      whenPage(model, 'Idle', () => ({
        model: withPage(model, Page.Choosing({ kind: 'serial' })),
        commands: [ChooseSerial()],
      })),

    ClickedDescribeUsb: () =>
      whenPage(model, 'Idle', () => ({
        model: withPage(model, Page.Choosing({ kind: 'usb' })),
        commands: [ChooseUsb()],
      })),

    ClickedChooseAgain: () =>
      whenPage(model, 'Live', live =>
        withSerial(model, live, port =>
          port.state._tag === 'Failed'
            ? { model, commands: [ChooseSerialAgain()] }
            : ignore(model),
        ),
      ),

    ClickedCopyCode: () =>
      whenPage(model, 'Live', live => ({
        model,
        commands: [CopyCode({ text: formatCode(live.session.code) })],
      })),

    ClickedDisconnect: () =>
      whenPage(model, 'Live', live => onDisconnect(model, live)),

    ClickedStartOver: () =>
      whenPage(model, 'Ended', () => ({
        model: withPage(model, Page.Idle({ notice: Option.none() })),
      })),

    ChoseSerial: ({ identity, session }) =>
      whenPage(model, 'Choosing', () => {
        const port = newPort(identity)
        return {
          model: startLive(model, { session, source: Source.Serial({ port }) }),
          commands: [SaveSession({ stored: storedOf(session, port) })],
        }
      }),

    ChoseUsb: ({ descriptor, session }) =>
      whenPage(model, 'Choosing', () => ({
        model: startLive(model, {
          session,
          source: Source.Usb({ descriptor }),
        }),
      })),

    ChoseSerialAgain: ({ identity }) =>
      whenPage(model, 'Live', live =>
        onChoseSerialAgain(model, live, identity),
      ),

    CancelledChooser: () =>
      whenPage(model, 'Choosing', () => ({
        model: withPage(model, Page.Idle({ notice: Option.none() })),
      })),

    FailedChooser: ({ reason }) =>
      whenPage(model, 'Choosing', () => ({
        model: withPage(model, Page.Idle({ notice: Option.some(reason) })),
      })),

    MintedSession: ({ session }) =>
      whenPage(model, 'Live', live => onMintedSession(model, live, session)),

    JoinedRelay: () =>
      whenPage(model, 'Live', live => onJoinedRelay(model, live)),
    FailedDialRelay: () =>
      whenPage(model, 'Live', live => onFailedDial(model, live)),
    ClosedRelay: closed =>
      whenPage(model, 'Live', live => onClosedRelay(model, live, closed)),
    RetryRelayDue: ({ attempt }) =>
      whenPage(model, 'Live', live => onRetryDue(model, live, attempt)),
    ReceivedFromRelay: ({ message: received }) =>
      whenPage(model, 'Live', live => onRelayMessage(model, live, received)),
    ReleasedRelay: () => ignore(model),

    SaidBye: () =>
      whenPage(model, 'Live', live =>
        Relay.guards.Leaving(live.relay)
          ? ended(model, 'visitorDisconnected')
          : ignore(model),
      ),

    OpenedPort: () =>
      whenPage(model, 'Live', live => onOpenedPort(model, live)),
    LostPort: ({ attempt }) =>
      whenPage(model, 'Live', live => onLostPort(model, live, attempt)),
    FailedPort: ({ reason }) =>
      whenPage(model, 'Live', live => onFailedPort(model, live, reason)),

    ReleasedSerial: () => ignore(model),

    Traffic: sample =>
      whenPage(model, 'Live', live => ({
        model: withLive(model, live, { monitor: record(live.monitor, sample) }),
      })),

    FinishedReset: ({ id, outcome }) => ({
      model,
      commands: [
        SendControl({ messages: [{ _tag: 'ResetResult', id, outcome }] }),
      ],
    }),

    CompletedSendControl: () => ignore(model),
    CompletedSaveSession: () => ignore(model),
    CompletedClearSession: () => ignore(model),
    FailedCopyCode: () => ignore(model),

    CopiedCode: () =>
      whenPage(model, 'Live', live => ({
        model: withLive(model, live, { copied: true }),
      })),
  })

const startPage = (flags: Flags): Page =>
  Option.match(detectMissing(flags.environment), {
    onSome: missing => Page.Unsupported({ missing }),
    onNone: () =>
      Option.match(flags.stored, {
        onNone: () => Page.Idle({ notice: Option.none() }),
        onSome: stored =>
          Page.Live({
            session: stored.session,
            source: Source.Serial({ port: recoveredPort(stored) }),
            relay: Relay.Dialing({ attempt: 1, failures: 0 }),
            monitor: emptyMonitor,
            copied: false,
          }),
      }),
  })

export const init = (flags: Flags): UpdateReturn => ({
  model: {
    canDescribeUsb: flags.environment.hasWebUsb,
    page: startPage(flags),
  },
})
