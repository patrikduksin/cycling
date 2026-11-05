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

import type { Session } from './model.ts'

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
