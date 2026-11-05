import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import { RequestId } from './ids.ts'
import {
  AgentMsg,
  BrowserMsg,
  RelayMsg,
  ToAgent,
  ToBrowser,
} from './messages.ts'
import { KEEPALIVE, MAX_FRAME_BYTES } from './wire.ts'

/**
 * A binary message's first byte names its channel, so what a frame means
 * never depends on a mode both ends learn at different times:
 *
 *   0x00 | device bytes                                   Console: what the pty sees
 *   0x01 | id u32 LE | offset u32 LE | payload bytes      OpData: one op's bulk data
 *
 * The sender's role gives the direction: OpData from the agent is a write
 * payload, from the browser read data. The relay checks only the size.
 */
export type Binary =
  | { readonly _tag: 'Console'; readonly bytes: Uint8Array }
  | {
      readonly _tag: 'OpData'
      readonly id: RequestId
      /** Offset of `bytes[0]` within the op's region. */
      readonly offset: number
      readonly bytes: Uint8Array
    }
export type OpData = Extract<Binary, { readonly _tag: 'OpData' }>

const CONSOLE = 0x00
const OP_DATA = 0x01
const OP_DATA_HEADER_BYTES = 9

/** OpData payload per frame: MAX_FRAME_BYTES less the header, rounded down to 1 KiB. */
export const OP_CHUNK_BYTES = 15 * 1024

/**
 * One WebSocket message: a binary channel frame, one JSON control object, or
 * the relay's pong. One ordered socket per role means control is never
 * reordered against bytes.
 */
export type Frame<C> =
  | Binary
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

const malformed = () => new ProtocolViolation({ reason: 'malformed' })

export const decodeBinary = (
  raw: Uint8Array,
): Effect.Effect<Binary, ProtocolViolation> => {
  if (raw.byteLength > MAX_FRAME_BYTES) {
    return Effect.fail(new ProtocolViolation({ reason: 'tooLarge' }))
  }
  if (raw[0] === CONSOLE) {
    return Effect.succeed({ _tag: 'Console', bytes: raw.subarray(1) })
  }
  if (raw[0] !== OP_DATA || raw.byteLength < OP_DATA_HEADER_BYTES) {
    return Effect.fail(malformed())
  }
  const view = new DataView(raw.buffer, raw.byteOffset, raw.byteLength)
  return Effect.succeed({
    _tag: 'OpData',
    id: RequestId.make(view.getUint32(1, true)),
    offset: view.getUint32(5, true),
    bytes: raw.subarray(OP_DATA_HEADER_BYTES),
  })
}

export const encodeBinary = (binary: Binary): Uint8Array => {
  if (binary._tag === 'Console') {
    const out = new Uint8Array(binary.bytes.byteLength + 1)
    out[0] = CONSOLE
    out.set(binary.bytes, 1)
    return out
  }
  const out = new Uint8Array(binary.bytes.byteLength + OP_DATA_HEADER_BYTES)
  const view = new DataView(out.buffer)
  view.setUint8(0, OP_DATA)
  view.setUint32(1, binary.id, true)
  view.setUint32(5, binary.offset, true)
  out.set(binary.bytes, OP_DATA_HEADER_BYTES)
  return out
}

/** One op's payload as OpData frames in offset order, from `start`; the only producer of OpData on either side. */
export const opDataFrames = function* (
  id: RequestId,
  payload: Uint8Array,
  start = 0,
): Iterable<OpData> {
  for (let at = 0; at < payload.byteLength; at += OP_CHUNK_BYTES) {
    yield {
      _tag: 'OpData',
      id,
      offset: start + at,
      bytes: payload.subarray(at, at + OP_CHUNK_BYTES),
    }
  }
}

/** One op's payload being put back together. Complete when `received === size`. */
export interface Assembly {
  readonly size: number
  readonly received: number
  readonly chunks: ReadonlyArray<Uint8Array>
}

export const emptyAssembly = (size: number): Assembly => ({
  size,
  received: 0,
  chunks: [],
})

/** None the moment a frame leaves a hole, overlaps, or runs past `size`. */
export const assemble = (
  assembly: Assembly,
  frame: { readonly offset: number; readonly bytes: Uint8Array },
): Option.Option<Assembly> =>
  frame.offset !== assembly.received ||
  assembly.received + frame.bytes.byteLength > assembly.size
    ? Option.none()
    : Option.some({
        size: assembly.size,
        received: assembly.received + frame.bytes.byteLength,
        chunks: [...assembly.chunks, frame.bytes],
      })

export const assembled = (assembly: Assembly): Uint8Array => {
  const out = new Uint8Array(assembly.received)
  let offset = 0
  for (const chunk of assembly.chunks) {
    out.set(chunk, offset)
    offset += chunk.byteLength
  }
  return out
}

const decoder = <S extends Schema.Codec<unknown, string>>(schema: S) => {
  const decode = Schema.decodeUnknownEffect(schema, STRICT)
  return (raw: Raw): Effect.Effect<Frame<S['Type']>, ProtocolViolation> => {
    if (typeof raw !== 'string') {
      return decodeBinary(raw instanceof Uint8Array ? raw : new Uint8Array(raw))
    }
    if (raw === KEEPALIVE.pong) {
      return Effect.succeed({ _tag: 'Pong' })
    }
    return decode(raw).pipe(
      Effect.map((message): Frame<S['Type']> => ({ _tag: 'Control', message })),
      Effect.mapError(malformed),
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
