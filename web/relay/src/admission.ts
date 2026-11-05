import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import {
  CloseCode,
  type Code,
  parseOffered,
  PROTOCOL_VERSION,
  type SessionPath,
} from '@cycling/protocol'

/** The shortest agent token the relay will hold: a shorter secret guards nothing. */
export const AGENT_TOKEN_MIN_LENGTH = 32

export const AgentToken = Schema.String.check(
  Schema.isMinLength(AGENT_TOKEN_MIN_LENGTH),
).pipe(Schema.brand('AgentToken'))
export type AgentToken = typeof AgentToken.Type

/** None for a token that is missing, empty or too short to be a secret. */
export const parseAgentToken = (
  secret: Option.Option<string>,
): Option.Option<AgentToken> =>
  secret.pipe(Option.flatMap(Schema.decodeUnknownOption(AgentToken)))

export interface OriginPolicy {
  readonly allowed: ReadonlyArray<string>
  readonly allowLocal: boolean
}

const LOCAL_ORIGIN = /^http:\/\/(localhost|127\.0\.0\.1)(:\d{1,5})?$/

export const parseOrigins = (csv: string): ReadonlyArray<string> =>
  csv
    .split(',')
    .map(origin => origin.trim())
    .filter(origin => origin.length > 0)

export const originAllowed = (
  origin: string | undefined,
  policy: OriginPolicy,
): boolean =>
  origin !== undefined &&
  (policy.allowed.includes(origin) ||
    (policy.allowLocal && LOCAL_ORIGIN.test(origin)))

const toHex = (bytes: ArrayBuffer): string =>
  Array.from(new Uint8Array(bytes), byte =>
    byte.toString(16).padStart(2, '0'),
  ).join('')

const digest = (text: string): Effect.Effect<ArrayBuffer> =>
  Effect.tryPromise(() =>
    crypto.subtle.digest('SHA-256', new TextEncoder().encode(text)),
  ).pipe(Effect.orDie)

export const sha256Hex = (text: string): Effect.Effect<string> =>
  digest(text).pipe(Effect.map(toHex))

/**
 * Compares two secrets in constant time by comparing their digests, so
 * neither the content nor the length of the expected secret leaks.
 */
export const secretsEqual = (
  offered: string,
  expected: string,
): Effect.Effect<boolean> =>
  Effect.all([digest(offered), digest(expected)]).pipe(
    Effect.map(([left, right]) => {
      const a = new Uint8Array(left)
      const b = new Uint8Array(right)
      let difference = 0
      for (let index = 0; index < a.length; index++) {
        difference |= (a[index] ?? 0) ^ (b[index] ?? 0)
      }
      return difference === 0
    }),
  )

export interface Credentials {
  /** None when the secret is unusable: every agent is then refused, none admitted. */
  readonly agentToken: Option.Option<AgentToken>
  readonly origins: OriginPolicy
}

export interface Request {
  readonly route: SessionPath
  readonly protocolHeader: string | null
  readonly origin: string | undefined
}

export type Admission =
  | {
      readonly _tag: 'Admit'
      readonly code: Code
      readonly role: 'agent'
    }
  | {
      readonly _tag: 'Admit'
      readonly code: Code
      readonly role: 'browser'
      readonly ownerKeyHash: string
    }
  | {
      readonly _tag: 'Refuse'
      readonly close: CloseCode
      /** What to echo as the selected subprotocol so the browser completes the handshake. */
      readonly echo: Option.Option<string>
    }

/**
 * Everything the Worker checks before a Durable Object is touched, in the
 * order that gives a client the most useful close code. Rate limiting is the
 * caller's, after this passes.
 */
export const admit = (
  request: Request,
  credentials: Credentials,
): Effect.Effect<Admission> => {
  const { route, protocolHeader, origin } = request
  const offered = parseOffered(protocolHeader)
  const refuse = (close: CloseCode): Admission => ({
    _tag: 'Refuse',
    close,
    echo: offered.selected,
  })
  if (route.version !== PROTOCOL_VERSION || !offered.version) {
    return Effect.succeed(refuse(CloseCode.unsupportedVersion))
  }
  const { code } = route
  if (Option.isNone(code)) {
    return Effect.succeed(refuse(CloseCode.notFound))
  }
  if (route.role === 'agent') {
    return Option.match(
      Option.all({ offered: offered.token, expected: credentials.agentToken }),
      {
        onNone: () => Effect.succeed(refuse(CloseCode.unauthorized)),
        onSome: ({ offered: token, expected }) =>
          secretsEqual(token, expected).pipe(
            Effect.map((equal): Admission =>
              equal
                ? { _tag: 'Admit', role: 'agent', code: code.value }
                : refuse(CloseCode.unauthorized),
            ),
          ),
      },
    )
  }
  if (
    Option.isNone(offered.ownerKey) ||
    !originAllowed(origin, credentials.origins)
  ) {
    return Effect.succeed(refuse(CloseCode.unauthorized))
  }
  return sha256Hex(offered.ownerKey.value).pipe(
    Effect.map((ownerKeyHash): Admission => ({
      _tag: 'Admit',
      role: 'browser',
      code: code.value,
      ownerKeyHash,
    })),
  )
}
