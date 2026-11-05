import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import { defineTaggedUnion } from 'foldkit/schema'

import {
  Baud,
  Code,
  EndReason,
  Epoch,
  OwnerKey,
  PortFailureReason,
  RequestId,
  SerialIdentity,
  UsbDescriptor,
} from '@cycling/protocol'

export const DEFAULT_BAUD = Schema.decodeSync(Baud)(115_200)

export const Session = Schema.Struct({ code: Code, ownerKey: OwnerKey })
export type Session = typeof Session.Type

/** What survives a reload: enough to dial the same session, find the same granted port and keep epochs counting up. */
export const StoredSession = Schema.Struct({
  session: Session,
  identity: SerialIdentity,
  baud: Baud,
  epoch: Epoch,
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
 * The protocol's `PortState` minus `Lost`: the page never rests there, it goes
 * straight to `Reacquiring` and reports both on the wire.
 */
export const PortPhase = Schema.Union([
  Schema.TaggedStruct('Opening', {}),
  Schema.TaggedStruct('Open', { baud: Baud }),
  Schema.TaggedStruct('Reacquiring', {}),
  Schema.TaggedStruct('Failed', { reason: PortFailureReason }),
])
export type PortPhase = typeof PortPhase.Type

/**
 * `attempt` is the serial ManagedResource's key. Attempt 0 opens a port the
 * chooser just granted; any later attempt waits for the same port to come
 * back.
 */
export const Port = Schema.Struct({
  identity: SerialIdentity,
  baud: Baud,
  attempt: Schema.Int,
  epoch: Epoch,
  state: PortPhase,
  pendingLineCoding: Schema.Option(RequestId),
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

export const livePort = (model: Model): Option.Option<Port> =>
  livePage(model).pipe(
    Option.map(live => live.source),
    Option.filter(Source.guards.Serial),
    Option.map(source => source.port),
  )
