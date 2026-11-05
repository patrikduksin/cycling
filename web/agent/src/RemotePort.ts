import * as Effect from 'effect/Effect'
import * as Filter from 'effect/Filter'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'

import type {
  Device,
  Op,
  OpOutcome,
  PortStatus,
  RequestId,
  ToAgent,
} from '@cycling/protocol'

import type { Ending, OpTransportError } from './errors.ts'

export type Link =
  | { readonly _tag: 'Connecting' }
  | { readonly _tag: 'Joined'; readonly browserPresent: boolean }
  | { readonly _tag: 'Redialing' }
  | { readonly _tag: 'Over'; readonly ending: Ending }

export interface Traffic {
  readonly fromDevice: number
  readonly toDevice: number
  /** Bytes written while the relay or the browser port was down. */
  readonly dropped: number
}

export interface RemoteStatus {
  readonly link: Link
  readonly device: Option.Option<Device>
  readonly port: Option.Option<PortStatus>
  readonly expiresAt: number
  readonly traffic: Traffic
}

/** What one op produces, in the order the page sent it; `Result` is last. */
export type OpEvent =
  | {
      readonly _tag: 'Data'
      readonly offset: number
      readonly bytes: Uint8Array
    }
  | { readonly _tag: 'Progress'; readonly done: number; readonly total: number }
  | { readonly _tag: 'Result'; readonly outcome: OpOutcome }

/** An op as the agent sends it: a Write carries its payload, nothing else has one. */
export type OpRequest =
  | { readonly op: Exclude<Op, { readonly _tag: 'Write' }> }
  | {
      readonly op: Extract<Op, { readonly _tag: 'Write' }>
      readonly payload: Uint8Array
    }

/**
 * The agent's whole view of a visitor's device. Redial, request
 * epochs and the relay's framing live behind these four members;
 * nothing that uses a port sees a wire message.
 */
export interface RemotePort {
  /** The announced device; waits for the browser to announce one. */
  readonly device: Effect.Effect<Device>
  /** Device to agent bytes. Hot: bytes nobody is reading are gone. */
  readonly output: Stream.Stream<Uint8Array>
  /** Device bytes from the moment this runs, kept for the stream while the scope lives. */
  readonly listen: Effect.Effect<Stream.Stream<Uint8Array>, never, Scope.Scope>
  /** Agent to device bytes. False when they were dropped because the relay or the port is down. */
  readonly write: (bytes: Uint8Array) => Effect.Effect<boolean>
  readonly status: SubscriptionRef.SubscriptionRef<RemoteStatus>
  /** A request id no op of this session has used. */
  readonly nextId: Effect.Effect<RequestId>
  /**
   * Sends one op under `id`, then a Write's payload as its data frames, and
   * streams what the page answers. Sending the same id again is a resend:
   * the page joins the op still running under it, or answers it again.
   * Fails `RelayDown` when the relay drops or is down, and `OpStalled` after
   * `OP_STALL_MS` with neither progress nor a result.
   */
  readonly op: (
    id: RequestId,
    request: OpRequest,
  ) => Stream.Stream<OpEvent, OpTransportError>
}

export const isPortOpen = (status: RemoteStatus): boolean =>
  status.link._tag === 'Joined' &&
  Option.exists(status.port, port => port.state._tag === 'Open')

export const openBaud = (status: RemoteStatus): Option.Option<number> =>
  Option.flatMap(status.port, ({ state }) =>
    state._tag === 'Open' ? Option.some(state.baud) : Option.none(),
  )

export const applyMessage = (
  status: RemoteStatus,
  message: ToAgent,
): RemoteStatus =>
  Match.valueTags(message, {
    Welcome: ({ snapshot }): RemoteStatus => ({
      ...status,
      link: { _tag: 'Joined', browserPresent: snapshot.browserPresent },
      device: Option.fromNullishOr(snapshot.device),
      port: Option.fromNullishOr(snapshot.port),
      expiresAt: snapshot.expiresAt,
    }),
    PeerPresence: ({ role, present }): RemoteStatus =>
      role === 'browser' && status.link._tag === 'Joined'
        ? { ...status, link: { _tag: 'Joined', browserPresent: present } }
        : status,
    DeviceAnnounced: ({ device }): RemoteStatus => ({
      ...status,
      device: Option.some(device),
    }),
    PortChanged: ({ status: port }): RemoteStatus => ({
      ...status,
      port: Option.some(port),
    }),
    OpProgress: () => status,
    OpResult: () => status,
    Bye: () => status,
  })

export const firstMatch = <S, A>(
  ref: SubscriptionRef.SubscriptionRef<S>,
  pick: (status: S) => Option.Option<A>,
): Effect.Effect<A> =>
  SubscriptionRef.changes(ref).pipe(
    Stream.filterMap(Filter.fromPredicateOption(pick)),
    Stream.runHead,
    Effect.flatMap(
      Option.match({ onNone: () => Effect.never, onSome: Effect.succeed }),
    ),
  )

export const over = (port: RemotePort): Effect.Effect<Ending> =>
  firstMatch(port.status, status =>
    status.link._tag === 'Over'
      ? Option.some(status.link.ending)
      : Option.none(),
  )
