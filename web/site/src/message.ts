import * as Schema from 'effect/Schema'
import { defineMessageUnion } from 'foldkit/message'

import {
  Lease,
  MAX_DETAIL_CHARS,
  Md5,
  OpFailure,
  PortFailureReason,
  RequestId,
  SerialIdentity,
  ToBrowser,
  UsbDescriptor,
} from '@cycling/protocol'

import { EntryFailure } from './links.ts'
import { Direction, Layout, Session } from './model.ts'

/**
 * How an op ended on the page, as `update` may see it: the wire's
 * `OpOutcome` without the chip facts. `Entered` is answered from the session
 * itself, so the MAC never reaches `update`.
 */
export const Finished = Schema.Union([
  Schema.TaggedStruct('Entered', {}),
  Schema.TaggedStruct('Read', { md5: Md5 }),
  Schema.TaggedStruct('Written', { md5: Md5 }),
  Schema.TaggedStruct('Digest', { md5: Md5 }),
  Schema.TaggedStruct('Rebooting', {}),
  Schema.TaggedStruct('Failed', {
    reason: OpFailure,
    detail: Schema.String.check(Schema.isMaxLength(MAX_DETAIL_CHARS)),
  }),
])
export type Finished = typeof Finished.Type

/**
 * Facts, not commands. Device bytes reach `update` only as `Traffic`, one
 * Message per direction per 100 ms, so `update` runs at human rates.
 */
export const Message = defineMessageUnion({
  ClickedConnectDevice: {},
  ClickedDescribeUsb: {},
  ClickedChooseAgain: {},
  ClickedCopyCode: {},
  ClickedDisconnect: {},
  ClickedStartOver: {},
  ClickedRestartDevice: {},
  ClickedAllowWrites: {},
  ClickedDeclineWrite: {},
  ClickedStopAndDisconnect: {},
  ClickedKeepGoing: {},

  ChoseSerial: { identity: SerialIdentity, session: Session },
  ChoseUsb: { descriptor: UsbDescriptor, session: Session },
  ChoseSerialAgain: { identity: SerialIdentity },
  CancelledChooser: {},
  FailedChooser: { reason: Schema.String },
  MintedSession: { session: Session },

  JoinedRelay: {},
  FailedDialRelay: {},
  ClosedRelay: { attempt: Schema.Int, code: Schema.Int },
  RetryRelayDue: { attempt: Schema.Int },
  ReceivedFromRelay: { message: ToBrowser },
  ReleasedRelay: {},
  SaidBye: {},

  OpenedPort: {},
  LostPort: { attempt: Schema.Int },
  FailedPort: { reason: PortFailureReason },
  ReleasedSerial: {},

  /** The flasher holds the port; `layout` is what the device's own tables say. */
  EnteredFlasher: { layout: Schema.Option(Layout) },
  FailedEntry: { reason: EntryFailure, detail: Schema.String },
  ProgressedOp: {
    attempt: Schema.Int,
    id: RequestId,
    done: Schema.Int,
    total: Schema.Int,
  },
  FinishedOp: { attempt: Schema.Int, id: RequestId, finished: Finished },
  AgentAwayElapsed: { lease: Lease, departures: Schema.Int },

  Traffic: {
    direction: Direction,
    bytes: Schema.Uint8Array,
    dropped: Schema.Int,
  },

  CompletedSendControl: {},
  CompletedSaveSession: {},
  CompletedClearSession: {},
  CopiedCode: {},
  FailedCopyCode: {},
})
export type Message = typeof Message.Type
