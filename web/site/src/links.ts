import * as Context from 'effect/Context'
import type * as Effect from 'effect/Effect'
import type * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import type * as Stream from 'effect/Stream'

import type {
  Baud,
  RelayDialError,
  RelaySocket,
  SerialIdentity,
  UsbDescriptor,
} from '@cycling/protocol'

import type { OpenFlasher, PageSession } from './adapters/flasher.ts'
import type { Inbox } from './flasher/inbox.ts'
import type { Layout, Session } from './model.ts'

/**
 * The hardware edge as plain interfaces, so the data plane and the
 * transition table run against in-memory fakes under `bun test` and the dev
 * build can swap in a fake device. Only the adapters under `adapters/` touch
 * `navigator.serial`, `navigator.usb` or the relay socket.
 */

export class SerialLinkLost extends Schema.TaggedError<
  SerialLinkLost,
  { readonly brand: unique symbol }
>()('SerialLinkLost', {}) {}

export class SerialOpenRefused extends Schema.TaggedError<
  SerialOpenRefused,
  { readonly brand: unique symbol }
>()('SerialOpenRefused', { reason: Schema.String }) {}

export class ReacquireTimeout extends Schema.TaggedError<
  ReacquireTimeout,
  { readonly brand: unique symbol }
>()('ReacquireTimeout', {}) {}

export class ChooserCancelled extends Schema.TaggedError<
  ChooserCancelled,
  { readonly brand: unique symbol }
>()('ChooserCancelled', {}) {}

export class ChooserFailed extends Schema.TaggedError<
  ChooserFailed,
  { readonly brand: unique symbol }
>()('ChooserFailed', { reason: Schema.String }) {}

/** Why the flasher could not take the port; `portLost` when the device went away during entry, as a re-enumeration does. */
export const EntryFailure = Schema.Literals([
  'portLost',
  'entryFailed',
  'unsupportedChip',
])
export type EntryFailure = typeof EntryFailure.Type

export class EntryFailed extends Schema.TaggedError<
  EntryFailed,
  { readonly brand: unique symbol }
>()('EntryFailed', { reason: EntryFailure, detail: Schema.String }) {}

export interface SerialLink {
  /** Chunks as the device sends them; fails with `SerialLinkLost` on unplug or reset. */
  readonly input: Stream.Stream<Uint8Array, SerialLinkLost>
  readonly write: (bytes: Uint8Array) => Effect.Effect<void, SerialLinkLost>
}

export interface SerialAdapter {
  /** Needs transient user activation, so it must be the first thing a click's Command does. */
  readonly choose: Effect.Effect<
    SerialIdentity,
    ChooserCancelled | ChooserFailed
  >
  /**
   * Opens the already-granted port with this identity, waiting up to `waitMs`
   * for it to appear (0 means it must be there now). Closing the Scope closes
   * the port.
   */
  readonly acquire: (options: {
    readonly identity: SerialIdentity
    readonly baud: Baud
    readonly waitMs: number
  }) => Effect.Effect<
    SerialLink,
    SerialOpenRefused | ReacquireTimeout,
    Scope.Scope
  >
  /**
   * The granted port with this identity, closed, for the flasher to open
   * itself; waits up to `waitMs` for it to come back like `acquire`.
   */
  readonly granted: (options: {
    readonly identity: SerialIdentity
    readonly waitMs: number
  }) => Effect.Effect<SerialPort, ReacquireTimeout>
}

/** What the page uses of `@cycling/flasher`; tests and `?fake` bring their own. */
export interface FlasherAdapter {
  readonly open: OpenFlasher
  readonly md5Hex: (bytes: Uint8Array) => string
}

/**
 * The one owner of the SerialPort: the console's byte link, or a flasher
 * session with the inbox its write payloads land in. Switching between them
 * is a re-key, so the old one is closed before the new one opens.
 */
export type PortLink =
  | { readonly _tag: 'Console'; readonly link: SerialLink }
  | {
      readonly _tag: 'Flasher'
      readonly session: PageSession
      readonly inbox: Inbox
      readonly layout: Option.Option<Layout>
    }

export interface UsbAdapter {
  readonly request: Effect.Effect<
    UsbDescriptor,
    ChooserCancelled | ChooserFailed
  >
}

/** The browser's end of the relay socket, from the protocol's `relaySocket`. */
export type RelayLink = RelaySocket<'browser'>

export interface RelayAdapter {
  readonly dial: (
    session: Session,
  ) => Effect.Effect<RelayLink, RelayDialError, Scope.Scope>
}

export interface Adapters {
  readonly serial: SerialAdapter
  readonly flasher: FlasherAdapter
  readonly usb: UsbAdapter
  /** None when the build has no relay address; the page then stops at its configuration screen. */
  readonly relay: Option.Option<RelayAdapter>
}

export interface ChooserService {
  readonly serial: SerialAdapter['choose']
  readonly usb: UsbAdapter['request']
}

/** What Commands see of the adapters; the rest reach the runtime through ManagedResources. */
export class Chooser extends Context.Service<Chooser, ChooserService>()(
  'site/Chooser',
) {}
