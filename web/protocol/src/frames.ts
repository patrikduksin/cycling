import * as Effect from 'effect/Effect'
import * as Schema from 'effect/Schema'

import {
  AgentMsg,
  BrowserMsg,
  RelayMsg,
  ToAgent,
  ToBrowser,
} from './messages.ts'
import { KEEPALIVE, MAX_FRAME_BYTES } from './wire.ts'

/**
 * One WebSocket message. Binary is raw device bytes with no header, in the
 * direction the sender's role implies; text is one JSON control object. One
 * ordered socket per role means control is never reordered against bytes.
 */
export type Frame<C> =
  | { readonly _tag: 'Bytes'; readonly bytes: Uint8Array }
  | { readonly _tag: 'Control'; readonly message: C }
  | { readonly _tag: 'Pong' }

export class ProtocolViolation extends Schema.TaggedError<
  ProtocolViolation,
  { readonly brand: unique symbol }
>()('ProtocolViolation', {
  reason: Schema.Literals(['malformed', 'tooLarge']),
}) {}

export type Raw = string | ArrayBuffer | Uint8Array

const STRICT = { onExcessProperty: 'error' } as const

const decoder = <S extends Schema.Codec<unknown, string>>(schema: S) => {
  const decode = Schema.decodeUnknownEffect(schema, STRICT)
  return (raw: Raw): Effect.Effect<Frame<S['Type']>, ProtocolViolation> => {
    if (typeof raw !== 'string') {
      const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw)
      return bytes.byteLength > MAX_FRAME_BYTES
        ? Effect.fail(new ProtocolViolation({ reason: 'tooLarge' }))
        : Effect.succeed({ _tag: 'Bytes', bytes })
    }
    if (raw === KEEPALIVE.pong) {
      return Effect.succeed({ _tag: 'Pong' })
    }
    return decode(raw).pipe(
      Effect.map((message): Frame<S['Type']> => ({ _tag: 'Control', message })),
      Effect.mapError(() => new ProtocolViolation({ reason: 'malformed' })),
    )
  }
}

export const decodeToBrowser = decoder(Schema.fromJsonString(ToBrowser))
export const decodeToAgent = decoder(Schema.fromJsonString(ToAgent))

export const decodeFromBrowser = decoder(Schema.fromJsonString(BrowserMsg))
export const decodeFromAgent = decoder(Schema.fromJsonString(AgentMsg))

export const encodeBrowserMsg = Schema.encodeSync(
  Schema.fromJsonString(BrowserMsg),
)
export const encodeAgentMsg = Schema.encodeSync(Schema.fromJsonString(AgentMsg))
export const encodeRelayMsg = Schema.encodeSync(Schema.fromJsonString(RelayMsg))
