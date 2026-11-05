import * as Schema from 'effect/Schema'
import { defineMessageUnion } from 'foldkit/message'

import {
  PortFailureReason,
  RequestId,
  RequestOutcome,
  SerialIdentity,
  ToBrowser,
  UsbDescriptor,
} from '@cycling/protocol'

import { Direction, Session } from './model.ts'

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

  Traffic: {
    direction: Direction,
    bytes: Schema.Uint8Array,
    dropped: Schema.Int,
  },

  FinishedReset: { id: RequestId, outcome: RequestOutcome },
  CompletedSendControl: {},
  CompletedSaveSession: {},
  CompletedClearSession: {},
  CopiedCode: {},
  FailedCopyCode: {},
})
export type Message = typeof Message.Type
