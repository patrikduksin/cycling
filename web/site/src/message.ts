import * as Schema from 'effect/Schema'
import { defineMessageUnion } from 'foldkit/message'

import {
  Lease,
  PortFailureReason,
  RequestId,
  SerialIdentity,
  ToBrowser,
  UsbDescriptor,
} from '@cycling/protocol'

import { EntryFailure } from './links.ts'
import { Direction, Finished, Layout, Session } from './model.ts'

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
  EnteredFlasher: { layout: Layout },
  FailedEntry: { reason: EntryFailure, detail: Schema.String },
  ProgressedOp: {
    attempt: Schema.Int,
    id: RequestId,
    done: Schema.Int,
    total: Schema.Int,
  },
  FinishedOp: { attempt: Schema.Int, id: RequestId, finished: Finished },
  /** The hold sat with no op running for its expiry (`holdExpiry`). */
  ExpiredHold: { attempt: Schema.Int, lease: Lease, after: Schema.Int },

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
