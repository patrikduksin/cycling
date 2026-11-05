import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Schema from 'effect/Schema'
import { Command } from 'foldkit'

import {
  BrowserMsg,
  RESET_PLANS,
  RequestId,
  ResetKind,
  mintCode,
  mintOwnerKey,
} from '@cycling/protocol'

import { RelayHandle, SerialHandle } from './handles.ts'
import { type ChooserCancelled, type ChooserFailed, Chooser } from './links.ts'
import { Message } from './message.ts'
import { StoredSession } from './model.ts'
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

/** Said while the relay handle is still held, so `Bye` is on the wire before the socket closes. */
export const SayBye = Command.define('SayBye', {
  messages: [Message.SaidBye],
  execute: RelayHandle.get.pipe(
    Effect.flatMap(link => link.send({ _tag: 'Bye' })),
    Effect.ignore,
    Effect.as(Message.SaidBye()),
  ),
})

export const RunReset = Command.define('RunReset', {
  args: { id: RequestId, kind: ResetKind },
  messages: [Message.FinishedReset],
  execute: ({ id, kind }) =>
    SerialHandle.get.pipe(
      Effect.flatMap(link => link.runSignals(RESET_PLANS[kind])),
      Effect.map(() => Message.FinishedReset({ id, outcome: 'done' })),
      Effect.catchTags({
        ResourceNotAvailable: () =>
          Effect.succeed(Message.FinishedReset({ id, outcome: 'portNotOpen' })),
        SerialLinkLost: () =>
          Effect.succeed(Message.FinishedReset({ id, outcome: 'failed' })),
      }),
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
