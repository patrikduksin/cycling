import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import { defineTaggedUnion } from 'foldkit/schema'

import { Entry } from '@cycling/flasher'
import {
  Baud,
  Code,
  EndReason,
  Epoch,
  Lease,
  Op,
  OpOutcome,
  OwnerKey,
  PortFailureReason,
  Region,
  RequestId,
  SerialIdentity,
  UsbDescriptor,
} from '@cycling/protocol'

export const DEFAULT_BAUD = Schema.decodeSync(Baud)(115_200)

/** One entry of the device's own ESP-IDF partition table. */
export const Partition = Schema.Struct({
  label: Schema.String,
  type: Schema.Int,
  subtype: Schema.Int,
  address: Schema.Int,
  size: Schema.Int,
})
export type Partition = typeof Partition.Type

/**
 * What the page read from the device at entry, so it can name regions in the
 * visitor's words. `bootloader` is where the chip's family keeps its
 * second-stage bootloader; `partitions` is empty when the device has no
 * table the page could read; `bootSlot` is the app partition otadata
 * selects.
 */
export const Layout = Schema.Struct({
  bootloader: Schema.Int,
  partitions: Schema.Array(Partition),
  bootSlot: Schema.Option(Schema.String),
})
export type Layout = typeof Layout.Type

export const Session = Schema.Struct({ code: Code, ownerKey: OwnerKey })
export type Session = typeof Session.Type

/**
 * What survives a reload: enough to dial the same session, find the same
 * granted port and keep epochs counting up, and the visitor's Allow for
 * writes, which lasts the session.
 */
export const StoredSession = Schema.Struct({
  session: Session,
  identity: SerialIdentity,
  baud: Baud,
  epoch: Epoch,
  writesAllowed: Schema.Boolean,
})
export type StoredSession = typeof StoredSession.Type

export const Environment = Schema.Struct({
  hasWebSerial: Schema.Boolean,
  hasWebUsb: Schema.Boolean,
  isSecureContext: Schema.Boolean,
  relayConfigured: Schema.Boolean,
})
export type Environment = typeof Environment.Type

export const Flags = Schema.Struct({
  environment: Environment,
  stored: Schema.Option(StoredSession),
})
export type Flags = typeof Flags.Type

/**
 * `attempt` is part of the ManagedResource requirement, so bumping it makes
 * the runtime release the socket and dial again. `failures` counts
 * consecutive failed dials and only feeds the backoff delay.
 */
export const Relay = defineTaggedUnion({
  Dialing: { attempt: Schema.Int, failures: Schema.Int },
  Joined: { attempt: Schema.Int, agentPresent: Schema.Boolean },
  Backoff: { attempt: Schema.Int, failures: Schema.Int },
  Leaving: { attempt: Schema.Int },
})
export type Relay = typeof Relay.Type

/**
 * How an op ended on the page, as `update` may see it: the wire's
 * `OpOutcome` without the chip facts or `Released`. `Entered` is answered
 * from the session itself, so the MAC never reaches `update`.
 */
export const Finished = OpOutcome.mapMembers(
  ([, read, written, digest, rebooting, , failed]) =>
    [
      Schema.TaggedStruct('Entered', {}),
      read,
      written,
      digest,
      rebooting,
      failed,
    ] as const,
)
export type Finished = typeof Finished.Type

/** The last op the hold answered, so a resend of it is answered again rather than run again. */
export const Answered = Schema.Struct({ id: RequestId, finished: Finished })
export type Answered = typeof Answered.Type

/**
 * Who holds the port and how far along it is. The first four are the
 * console, the protocol's `PortState` minus `Lost` (the page never rests
 * there; it goes straight to `Reacquiring` and reports both on the wire).
 * `Entering` and `Flashing` are a hold: the agent's `lease` owns the device
 * from an `Enter` until a Reboot, a Release, a lost port, a failed op, the
 * visitor's Restart, or an idle hold expiring (`holdExpiry`).
 */
export const PortPhase = Schema.Union([
  Schema.TaggedStruct('Opening', {}),
  Schema.TaggedStruct('Open', { baud: Baud }),
  Schema.TaggedStruct('Reacquiring', {}),
  Schema.TaggedStruct('Failed', { reason: PortFailureReason }),
  Schema.TaggedStruct('Entering', { lease: Lease, entry: Entry }),
  Schema.TaggedStruct('Flashing', {
    lease: Lease,
    entry: Entry,
    layout: Layout,
    answered: Schema.Option(Answered),
  }),
])
export type PortPhase = typeof PortPhase.Type
export type Hold = Extract<
  PortPhase,
  { readonly _tag: 'Entering' | 'Flashing' }
>

/** Every op but Release, which is answered at once and never queued. */
export const RunnableOp = Op.mapMembers(
  ([enter, read, write, digest, reboot]) =>
    [enter, read, write, digest, reboot] as const,
)
export type RunnableOp = typeof RunnableOp.Type

/** An op the page accepted, with what the visitor sees of its progress. */
export const QueuedOp = Schema.Struct({
  id: RequestId,
  op: RunnableOp,
  done: Schema.Int,
  total: Schema.Int,
})
export type QueuedOp = typeof QueuedOp.Type

/** An op's kind, or the visitor's own Restart. */
export const ActivityKind = Schema.Literals([
  'Enter',
  'Read',
  'Write',
  'Digest',
  'Reboot',
  'Restart',
])
export type ActivityKind = typeof ActivityKind.Type

/** One line of what happened to the device, newest first in `Port.activity`. */
export const Activity = Schema.Struct({
  seq: Schema.Int,
  kind: ActivityKind,
  region: Schema.Option(Region),
  /** The device's own name for the region, when its table has one. */
  label: Schema.Option(Schema.String),
  result: Schema.Literals(['done', 'failed', 'stopped', 'declined']),
})
export type Activity = typeof Activity.Type

/**
 * `attempt` is part of the port ManagedResource's key. Attempt 0 opens a
 * port the chooser just granted; any later attempt waits for the same port
 * to come back. `ops` is the agent's queue: the head runs once the flasher
 * holds the port. It lives here rather than in the hold so an Enter queued
 * behind a Reboot survives the way back through the console.
 */
export const Port = Schema.Struct({
  identity: SerialIdentity,
  baud: Baud,
  attempt: Schema.Int,
  epoch: Epoch,
  state: PortPhase,
  ops: Schema.Array(QueuedOp),
  activity: Schema.Array(Activity),
})
export type Port = typeof Port.Type

export const Source = defineTaggedUnion({
  Serial: { port: Port },
  Usb: { descriptor: UsbDescriptor },
})
export type Source = typeof Source.Type

export const Direction = Schema.Literals(['fromDevice', 'toDevice'])
export type Direction = typeof Direction.Type

/** One monitor line, already formatted: the Model never holds raw bytes. */
export const Row = Schema.Struct({
  id: Schema.Int,
  direction: Direction,
  size: Schema.Int,
  hex: Schema.String,
  text: Schema.String,
  hiddenBytes: Schema.Int,
})
export type Row = typeof Row.Type

export const Monitor = Schema.Struct({
  rows: Schema.Array(Row),
  nextRowId: Schema.Int,
  bytesFromDevice: Schema.Int,
  bytesToDevice: Schema.Int,
  droppedFromDevice: Schema.Int,
  droppedToDevice: Schema.Int,
})
export type Monitor = typeof Monitor.Type

/**
 * Every close that ends the page, by its `closeMeaning`: an `Ended` reason,
 * `replaced`, or a refusal. A Disconnect click is `visitorDisconnected`, the
 * reason the relay gives for it.
 */
export const Ending = Schema.Literals([
  ...EndReason.literals,
  'replaced',
  'unauthorized',
  'notFound',
  'unsupportedVersion',
  'rateLimited',
  'protocolViolation',
])
export type Ending = typeof Ending.Type

export const Missing = Schema.Literals([
  'secureContext',
  'webSerial',
  'relayUrl',
])
export type Missing = typeof Missing.Type

export const Page = defineTaggedUnion({
  Unsupported: { missing: Missing },
  Idle: { notice: Schema.Option(Schema.String) },
  Choosing: { kind: Schema.Literals(['serial', 'usb']) },
  Live: {
    session: Session,
    source: Source,
    relay: Relay,
    monitor: Monitor,
    copied: Schema.Boolean,
    /** The visitor's Allow for writes; asked at the first Write of a session. */
    writesAllowed: Schema.Boolean,
    /** Disconnect was pressed during a write and is waiting for a second answer. */
    confirmingStop: Schema.Boolean,
  },
  Ended: { reason: Ending },
})
export type Page = typeof Page.Type
export type Live = Extract<Page, { readonly _tag: 'Live' }>

export const Model = Schema.Struct({
  canDescribeUsb: Schema.Boolean,
  page: Page,
})
export type Model = typeof Model.Type

export const livePage = (model: Model): Option.Option<Live> =>
  Option.some(model.page).pipe(Option.filter(Page.guards.Live))

/** The agent is on the relay with the page; a page that lost the relay cannot tell, and counts it gone. */
export const agentHere = (live: Live): boolean =>
  Relay.guards.Joined(live.relay) && live.relay.agentPresent

export const livePort = (model: Model): Option.Option<Port> =>
  livePage(model).pipe(
    Option.map(live => live.source),
    Option.filter(Source.guards.Serial),
    Option.map(source => source.port),
  )
