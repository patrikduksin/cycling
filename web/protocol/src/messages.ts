import * as Schema from 'effect/Schema'

import { Device } from './device.ts'
import { Baud, Epoch, RequestId, Role } from './ids.ts'
import { Op, OpOutcome } from './ops.ts'

export const PortFailureReason = Schema.Literals([
  'openRefused',
  'reacquireTimeout',
])
export type PortFailureReason = typeof PortFailureReason.Type

/**
 * Browser-side port lifecycle. Every USB reset walks Open, Lost, Reacquiring,
 * Open with a new epoch. `Flashing`: the page's flasher holds the port, so no
 * console bytes flow either way.
 */
export const PortState = Schema.Union([
  Schema.TaggedStruct('Opening', {}),
  Schema.TaggedStruct('Open', { baud: Baud }),
  Schema.TaggedStruct('Flashing', {}),
  Schema.TaggedStruct('Lost', {}),
  Schema.TaggedStruct('Reacquiring', {}),
  Schema.TaggedStruct('Failed', { reason: PortFailureReason }),
])
export type PortState = typeof PortState.Type

export const PortStatus = Schema.Struct({ epoch: Epoch, state: PortState })
export type PortStatus = typeof PortStatus.Type

/** Why a session ended; each is also the name of the close code both sockets get. */
export const EndReason = Schema.Literals([
  'visitorDisconnected',
  'visitorGone',
  'expired',
])
export type EndReason = typeof EndReason.Type

/** Text frames the browser sends. */
export const BrowserMsg = Schema.Union([
  Schema.TaggedStruct('DeviceAnnounced', { device: Device }),
  Schema.TaggedStruct('PortChanged', { status: PortStatus }),
  /** At least every `OP_PROGRESS_MS` while op `id` runs. */
  Schema.TaggedStruct('OpProgress', {
    id: RequestId,
    done: Schema.Int,
    total: Schema.Int,
  }),
  /** Exactly one per op the page accepted or refused. */
  Schema.TaggedStruct('OpResult', { id: RequestId, outcome: OpOutcome }),
  Schema.TaggedStruct('Bye', {}),
])
export type BrowserMsg = typeof BrowserMsg.Type

/** Text frames the agent sends. An agent leaving is a socket close, never the end of the session. */
export const AgentMsg = Schema.Union([
  Schema.TaggedStruct('Op', { id: RequestId, op: Op }),
])
export type AgentMsg = typeof AgentMsg.Type

/** What a late joiner needs; the relay stores exactly this, so Welcome is a read. */
export const Snapshot = Schema.Struct({
  device: Schema.NullOr(Device),
  port: Schema.NullOr(PortStatus),
  expiresAt: Schema.Int,
  browserPresent: Schema.Boolean,
  agentPresent: Schema.Boolean,
})
export type Snapshot = typeof Snapshot.Type

/** Text frames only the relay originates. */
export const RelayMsg = Schema.Union([
  Schema.TaggedStruct('Welcome', { role: Role, snapshot: Snapshot }),
  Schema.TaggedStruct('PeerPresence', { role: Role, present: Schema.Boolean }),
])
export type RelayMsg = typeof RelayMsg.Type

/** What each client may receive. A wrong-direction message fails to decode at the boundary. */
export const ToBrowser = Schema.Union([AgentMsg, RelayMsg])
export type ToBrowser = typeof ToBrowser.Type
export const ToAgent = Schema.Union([BrowserMsg, RelayMsg])
export type ToAgent = typeof ToAgent.Type
