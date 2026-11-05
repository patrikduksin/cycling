import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import { Command } from 'foldkit'

import {
  BrowserMsg,
  Chip,
  Lease,
  type OpOutcome,
  RequestId,
  mintCode,
  mintOwnerKey,
} from '@cycling/protocol'

import { PortHandle, RelayHandle } from './handles.ts'
import {
  type ChooserCancelled,
  type ChooserFailed,
  Chooser,
  type PortLink,
} from './links.ts'
import { Message } from './message.ts'
import { StoredSession } from './model.ts'
import { AGENT_GRACE_MS } from './policy.ts'
import { clearStoredSession, writeStoredSession } from './storage.ts'

const mintSession = Effect.sync(() => ({
  code: mintCode(crypto.getRandomValues(new Uint8Array(5))),
  ownerKey: mintOwnerKey(crypto.getRandomValues(new Uint8Array(16))),
}))

/** A closed or failed chooser is a Message too, so `update` can return to Idle. */
const chooserOutcome = <A extends Message>(
  chosen: Effect.Effect<A, ChooserCancelled | ChooserFailed, Chooser>,
) =>
  chosen.pipe(
    Effect.catchTags({
      ChooserCancelled: () => Effect.succeed(Message.CancelledChooser()),
      ChooserFailed: ({ reason }) =>
        Effect.succeed(Message.FailedChooser({ reason })),
    }),
  )

/**
 * The click's first action must be the chooser, with nothing awaited before
 * it, or Chrome drops the user activation `requestPort` needs. The session
 * is minted after the visitor has chosen.
 */
export const ChooseSerial = Command.define('ChooseSerial', {
  messages: [
    Message.ChoseSerial,
    Message.CancelledChooser,
    Message.FailedChooser,
  ],
  execute: chooserOutcome(
    Effect.gen(function* () {
      const chooser = yield* Chooser
      const identity = yield* chooser.serial
      const session = yield* mintSession
      return Message.ChoseSerial({ identity, session })
    }),
  ),
})

export const ChooseSerialAgain = Command.define('ChooseSerialAgain', {
  messages: [
    Message.ChoseSerialAgain,
    Message.CancelledChooser,
    Message.FailedChooser,
  ],
  execute: chooserOutcome(
    Chooser.use(chooser => chooser.serial).pipe(
      Effect.map(identity => Message.ChoseSerialAgain({ identity })),
    ),
  ),
})

export const ChooseUsb = Command.define('ChooseUsb', {
  messages: [Message.ChoseUsb, Message.CancelledChooser, Message.FailedChooser],
  execute: chooserOutcome(
    Effect.gen(function* () {
      const chooser = yield* Chooser
      const descriptor = yield* chooser.usb
      const session = yield* mintSession
      return Message.ChoseUsb({ descriptor, session })
    }),
  ),
})

export const MintSession = Command.define('MintSession', {
  messages: [Message.MintedSession],
  execute: mintSession.pipe(
    Effect.map(session => Message.MintedSession({ session })),
  ),
})

/** Sends in order, so a Lost never overtakes the Reacquiring that follows it. */
export const SendControl = Command.define('SendControl', {
  args: { messages: Schema.Array(BrowserMsg) },
  messages: [Message.CompletedSendControl],
  execute: ({ messages }) =>
    RelayHandle.get.pipe(
      Effect.flatMap(link =>
        Effect.forEach(messages, message => link.send(message), {
          discard: true,
        }),
      ),
      Effect.ignore,
      Effect.as(Message.CompletedSendControl()),
    ),
})

const decodeChip = Schema.decodeUnknownOption(Chip)

const enteredOutcome = (port: Option.Option<PortLink>): OpOutcome => {
  if (Option.isNone(port) || port.value._tag !== 'Flasher') {
    return { _tag: 'Failed', reason: 'portLost', detail: '' }
  }
  return Option.match(decodeChip(port.value.session.chip), {
    onSome: (chip): OpOutcome => ({ _tag: 'Entered', chip }),
    onNone: (): OpOutcome => ({
      _tag: 'Failed',
      reason: 'chipError',
      detail: 'The chip facts did not parse.',
    }),
  })
}

/**
 * Answers an Enter from the held session. The chip facts, the MAC among
 * them, go from the session to the relay here and never through `update`.
 */
export const SendEntered = Command.define('SendEntered', {
  args: { id: RequestId },
  messages: [Message.CompletedSendControl],
  execute: ({ id }) =>
    PortHandle.get.pipe(
      Effect.option,
      Effect.flatMap(port =>
        RelayHandle.get.pipe(
          Effect.flatMap(link =>
            link.send({ _tag: 'OpResult', id, outcome: enteredOutcome(port) }),
          ),
        ),
      ),
      Effect.ignore,
      Effect.as(Message.CompletedSendControl()),
    ),
})

/** The agent left during a hold; past this it is not coming back for it. */
export const WaitForAgent = Command.define('WaitForAgent', {
  args: { lease: Lease, departures: Schema.Int },
  messages: [Message.AgentAwayElapsed],
  execute: ({ lease, departures }) =>
    Effect.sleep(Duration.millis(AGENT_GRACE_MS)).pipe(
      Effect.as(Message.AgentAwayElapsed({ lease, departures })),
    ),
})

/** Said while the relay handle is still held, so `Bye` is on the wire before the socket closes. */
export const SayBye = Command.define('SayBye', {
  messages: [Message.SaidBye],
  execute: RelayHandle.get.pipe(
    Effect.flatMap(link => link.send({ _tag: 'Bye' })),
    Effect.ignore,
    Effect.as(Message.SaidBye()),
  ),
})

export const WaitRelayRetry = Command.define('WaitRelayRetry', {
  args: { attempt: Schema.Int, delayMs: Schema.Int },
  messages: [Message.RetryRelayDue],
  execute: ({ attempt, delayMs }) =>
    Effect.sleep(Duration.millis(delayMs)).pipe(
      Effect.as(Message.RetryRelayDue({ attempt })),
    ),
})

export const SaveSession = Command.define('SaveSession', {
  args: { stored: StoredSession },
  messages: [Message.CompletedSaveSession],
  execute: ({ stored }) =>
    writeStoredSession(stored).pipe(Effect.as(Message.CompletedSaveSession())),
})

export const ClearSession = Command.define('ClearSession', {
  messages: [Message.CompletedClearSession],
  execute: clearStoredSession.pipe(Effect.as(Message.CompletedClearSession())),
})

export const CopyCode = Command.define('CopyCode', {
  args: { text: Schema.String },
  messages: [Message.CopiedCode, Message.FailedCopyCode],
  execute: ({ text }) =>
    Effect.tryPromise(() => navigator.clipboard.writeText(text)).pipe(
      Effect.as(Message.CopiedCode()),
      Effect.orElseSucceed(() => Message.FailedCopyCode()),
    ),
})
