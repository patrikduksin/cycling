import * as Schema from 'effect/Schema'

import { Code, type EndReason, ResetKind } from '@cycling/protocol'

export class Unauthorized extends Schema.TaggedError<
  Unauthorized,
  { readonly brand: unique symbol }
>()('Unauthorized', {}) {}

export class SessionNotFound extends Schema.TaggedError<
  SessionNotFound,
  { readonly brand: unique symbol }
>()('SessionNotFound', {}) {}

export class UnsupportedVersion extends Schema.TaggedError<
  UnsupportedVersion,
  { readonly brand: unique symbol }
>()('UnsupportedVersion', {
  /** `protocolViolation`: the relay refused a frame this agent sent. */
  why: Schema.Literals(['unsupportedVersion', 'protocolViolation']),
}) {}

export class RelayUnreachable extends Schema.TaggedError<
  RelayUnreachable,
  { readonly brand: unique symbol }
>()('RelayUnreachable', {
  why: Schema.Union([
    Schema.TaggedStruct('Dial', {
      reason: Schema.Literals(['unreachable', 'subprotocol']),
      detail: Schema.String,
    }),
    Schema.TaggedStruct('Closed', { code: Schema.Int }),
  ]),
}) {}

/** Why the relay would not let this agent in, or later threw it out for good. */
export type AdmissionError =
  | Unauthorized
  | SessionNotFound
  | UnsupportedVersion
  | RelayUnreachable

/** How a session stops for good; the relay never redials past one of these. */
export type Ending =
  | { readonly _tag: 'Ended'; readonly reason: EndReason }
  | { readonly _tag: 'Replaced' }
  | { readonly _tag: 'Rejected'; readonly error: AdmissionError }

export class PortNotOpen extends Schema.TaggedError<
  PortNotOpen,
  { readonly brand: unique symbol }
>()('PortNotOpen', {}) {}

export class RequestFailed extends Schema.TaggedError<
  RequestFailed,
  { readonly brand: unique symbol }
>()('RequestFailed', {}) {}

export class RequestTimedOut extends Schema.TaggedError<
  RequestTimedOut,
  { readonly brand: unique symbol }
>()('RequestTimedOut', { ms: Schema.Int }) {}

export class RelayDown extends Schema.TaggedError<
  RelayDown,
  { readonly brand: unique symbol }
>()('RelayDown', {}) {}

/** Why a reset or line-coding request did not complete. */
export type RequestError =
  | PortNotOpen
  | RequestFailed
  | RequestTimedOut
  | RelayDown

export class LocalResourceError extends Schema.TaggedError<
  LocalResourceError,
  { readonly brand: unique symbol }
>()('LocalResourceError', {
  resource: Schema.Literals(['pty', 'rfc2217', 'control', 'sessionDir']),
  detail: Schema.String,
}) {}

export class BadCode extends Schema.TaggedError<
  BadCode,
  { readonly brand: unique symbol }
>()('BadCode', { input: Schema.String }) {}

export class PortNeverOpened extends Schema.TaggedError<
  PortNeverOpened,
  { readonly brand: unique symbol }
>()('PortNeverOpened', { seconds: Schema.Int, state: Schema.String }) {}

export class ControlRefused extends Schema.TaggedError<
  ControlRefused,
  { readonly brand: unique symbol }
>()('ControlRefused', {
  kind: ResetKind,
  why: Schema.Union([
    Schema.TaggedStruct('NoAgent', { code: Code }),
    /** The running agent's own words; empty when it closed without one. */
    Schema.TaggedStruct('Answered', { reply: Schema.String }),
  ]),
}) {}

/** Everything that stops `cycling remote <code>` before or instead of a session ending. */
export type SessionError =
  | AdmissionError
  | BadCode
  | LocalResourceError
  | PortNeverOpened
