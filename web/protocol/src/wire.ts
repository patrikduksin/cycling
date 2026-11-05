import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import {
  Code,
  OwnerKey,
  PROTOCOL_VERSION,
  type Role,
  Role as RoleSchema,
} from './ids.ts'
import type { EndReason } from './messages.ts'

/**
 * Why the relay closed a socket. Admission failures are also delivered this
 * way (the relay accepts the upgrade, then closes), because a browser cannot
 * read the HTTP status of a refused upgrade. A session that ends closes both
 * sockets with the code named after its `EndReason`.
 */
export const CloseCode = {
  visitorDisconnected: 4000,
  visitorGone: 4001,
  expired: 4408,
  unauthorized: 4401,
  notFound: 4404,
  codeInUse: 4409,
  replaced: 4410,
  unsupportedVersion: 4426,
  rateLimited: 4429,
  protocolViolation: 1008,
} as const satisfies Record<EndReason, number> & Record<string, number>
export type CloseCode = (typeof CloseCode)[keyof typeof CloseCode]
export type CloseName = keyof typeof CloseCode

export type Refusal =
  | 'unauthorized'
  | 'notFound'
  | 'unsupportedVersion'
  | 'rateLimited'
  | 'protocolViolation'

/**
 * What a close code tells a client to do. `Transient` is anything the relay
 * did not send on purpose (a dropped network, a Worker restart), so it is
 * worth a redial; `Remint` is a browser's code colliding with someone else's.
 */
export type CloseMeaning =
  | { readonly _tag: 'Transient' }
  | { readonly _tag: 'Ended'; readonly reason: EndReason }
  | { readonly _tag: 'Replaced' }
  | { readonly _tag: 'Refused'; readonly why: Refusal }
  | { readonly _tag: 'Remint' }

const ended = (reason: EndReason): CloseMeaning => ({ _tag: 'Ended', reason })
const refused = (why: Refusal): CloseMeaning => ({ _tag: 'Refused', why })

const MEANINGS: Record<CloseName, CloseMeaning> = {
  visitorDisconnected: ended('visitorDisconnected'),
  visitorGone: ended('visitorGone'),
  expired: ended('expired'),
  unauthorized: refused('unauthorized'),
  notFound: refused('notFound'),
  codeInUse: { _tag: 'Remint' },
  replaced: { _tag: 'Replaced' },
  unsupportedVersion: refused('unsupportedVersion'),
  rateLimited: refused('rateLimited'),
  protocolViolation: refused('protocolViolation'),
}

const isCloseName = (key: string): key is CloseName =>
  Object.hasOwn(CloseCode, key)

const NAMES: ReadonlyMap<number, CloseName> = new Map(
  Object.keys(CloseCode)
    .filter(isCloseName)
    .map(name => [CloseCode[name], name]),
)

/** The relay's name for a close code, or None for one it never sends. */
export const closeName = (code: number): Option.Option<CloseName> =>
  Option.fromNullishOr(NAMES.get(code))

export const closeMeaning = (code: number): CloseMeaning =>
  Option.match(closeName(code), {
    onNone: (): CloseMeaning => ({ _tag: 'Transient' }),
    onSome: name => MEANINGS[name],
  })

export const SUBPROTOCOL = `cycling.v${PROTOCOL_VERSION}`

export const sessionPath = (role: Role, code: Code): string =>
  `/v${PROTOCOL_VERSION}/${role}/${code}`

export interface SessionPath {
  readonly role: Role
  readonly version: number
  /** None when the segment is not a well-formed code: the relay answers notFound. */
  readonly code: Option.Option<Code>
}

const SESSION_PATH = /^\/v(\d+)\/(browser|agent)\/([^/]+)$/
const decodeRole = Schema.decodeUnknownOption(RoleSchema)
const decodeCode = Schema.decodeUnknownOption(Code)

/** The inverse of `sessionPath`; None for any path that is not a session's. */
export const parseSessionPath = (
  pathname: string,
): Option.Option<SessionPath> => {
  const [, version, role, code] = SESSION_PATH.exec(pathname) ?? []
  return Option.all({
    version: Option.fromNullishOr(version),
    role: decodeRole(role),
    code: Option.fromNullishOr(code),
  }).pipe(
    Option.map(parts => ({
      role: parts.role,
      version: Number(parts.version),
      code: decodeCode(parts.code),
    })),
  )
}

/**
 * Browsers cannot set WebSocket headers, so credentials ride in
 * `Sec-WebSocket-Protocol` next to the version token, and the relay echoes
 * only the version token.
 */
export const agentProtocols = (token: string): ReadonlyArray<string> => [
  SUBPROTOCOL,
  `auth.${token}`,
]
export const browserProtocols = (key: OwnerKey): ReadonlyArray<string> => [
  SUBPROTOCOL,
  `own.${key}`,
]

export interface Offered {
  readonly entries: ReadonlyArray<string>
  readonly version: boolean
  readonly token: Option.Option<string>
  readonly ownerKey: Option.Option<OwnerKey>
  /**
   * What the 101 must name. A browser fails the handshake unless the
   * response selects one of the offered tokens, so even a refusal echoes the
   * version token when it is offered, else the first token that is not a
   * credential (never an `auth.` or `own.` one).
   */
  readonly selected: Option.Option<string>
}

const CREDENTIAL = /^(auth|own)\./
const decodeOwnerKey = Schema.decodeUnknownOption(OwnerKey)

export const parseOffered = (header: string | null): Offered => {
  const entries = (header ?? '')
    .split(',')
    .map(entry => entry.trim())
    .filter(entry => entry.length > 0)
  const after = (prefix: string) =>
    Option.fromNullishOr(entries.find(entry => entry.startsWith(prefix))).pipe(
      Option.map(entry => entry.slice(prefix.length)),
    )
  const version = entries.includes(SUBPROTOCOL)
  return {
    entries,
    version,
    token: after('auth.'),
    ownerKey: after('own.').pipe(Option.flatMap(decodeOwnerKey)),
    selected: version
      ? Option.some(SUBPROTOCOL)
      : Option.fromNullishOr(entries.find(entry => !CREDENTIAL.test(entry))),
  }
}

export const COALESCE = { maxBytes: 4096, maxWaitMs: 10 } as const
/** Largest binary frame either client sends; the relay refuses bigger ones. */
export const MAX_FRAME_BYTES = 16 * 1024
/** Clients send `ping` this often; the relay answers `pong` without waking. */
export const KEEPALIVE = {
  ping: 'ping',
  pong: 'pong',
  intervalMs: 20_000,
} as const

export const BACKOFF = { baseMs: 500, maxMs: 10_000 } as const

/** The wait before redial number `failures` (1 for the first redial). */
export const backoffMs = (failures: number): number =>
  Math.min(BACKOFF.baseMs * 2 ** Math.max(failures - 1, 0), BACKOFF.maxMs)
