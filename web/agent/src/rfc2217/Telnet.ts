import * as Match from 'effect/Match'

export const IAC = 0xff
const DONT = 0xfe
const DO = 0xfd
const WONT = 0xfc
const WILL = 0xfb
const SB = 0xfa
const SE = 0xf0

export type Verb = 'WILL' | 'WONT' | 'DO' | 'DONT'

const VERB_BYTE: Readonly<Record<Verb, number>> = {
  WILL,
  WONT,
  DO,
  DONT,
}

const verbOf = (byte: number): Verb | null => {
  switch (byte) {
    case WILL:
      return 'WILL'
    case WONT:
      return 'WONT'
    case DO:
      return 'DO'
    case DONT:
      return 'DONT'
    default:
      return null
  }
}

/** One unit of a client's telnet stream. Data is already un-doubled; a subnegotiation payload starts with its option byte. */
export type Item =
  | { readonly _tag: 'Data'; readonly bytes: Uint8Array }
  | {
      readonly _tag: 'Negotiate'
      readonly verb: Verb
      readonly option: number
    }
  | { readonly _tag: 'Subnegotiation'; readonly payload: Uint8Array }
  | { readonly _tag: 'Command'; readonly command: number }

export type Parser =
  | { readonly _tag: 'Normal' }
  | { readonly _tag: 'Iac' }
  | { readonly _tag: 'Verb'; readonly verb: Verb }
  | { readonly _tag: 'Sub'; readonly payload: ReadonlyArray<number> }
  | { readonly _tag: 'SubIac'; readonly payload: ReadonlyArray<number> }

export const initialParser: Parser = { _tag: 'Normal' }

/** Pure and chunk-boundary agnostic: an IAC split across two reads parses the same as one read. */
export const parse = (
  start: Parser,
  chunk: Uint8Array,
): { readonly parser: Parser; readonly items: ReadonlyArray<Item> } => {
  const items: Array<Item> = []
  const data: Array<number> = []
  const flush = () => {
    if (data.length > 0) {
      items.push({ _tag: 'Data', bytes: Uint8Array.from(data) })
      data.length = 0
    }
  }
  let parser = start
  for (const byte of chunk) {
    const [next, emitted] = advance(parser, byte)
    parser = next
    if (typeof emitted === 'number') {
      data.push(emitted)
    } else if (emitted !== null) {
      flush()
      items.push(emitted)
    }
  }
  flush()
  return { parser, items }
}

/** One byte of the telnet state machine; a number is one data byte. */
const advance = (
  parser: Parser,
  byte: number,
): readonly [Parser, number | Item | null] =>
  Match.valueTags(parser, {
    Normal: (): readonly [Parser, number | null] =>
      byte === IAC ? [{ _tag: 'Iac' }, null] : [parser, byte],
    Iac: (): readonly [Parser, number | Item | null] => {
      const verb = verbOf(byte)
      if (byte === IAC) return [initialParser, IAC]
      if (byte === SB) return [{ _tag: 'Sub', payload: [] }, null]
      if (verb !== null) return [{ _tag: 'Verb', verb }, null]
      return [initialParser, { _tag: 'Command', command: byte }]
    },
    Verb: ({ verb }): readonly [Parser, Item] => [
      initialParser,
      { _tag: 'Negotiate', verb, option: byte },
    ],
    Sub: ({ payload }): readonly [Parser, null] =>
      byte === IAC
        ? [{ _tag: 'SubIac', payload }, null]
        : [{ _tag: 'Sub', payload: [...payload, byte] }, null],
    SubIac: ({ payload }): readonly [Parser, Item | null] =>
      byte === SE
        ? [
            initialParser,
            { _tag: 'Subnegotiation', payload: Uint8Array.from(payload) },
          ]
        : [{ _tag: 'Sub', payload: [...payload, byte] }, null],
  })

const doubleIac = (bytes: Uint8Array): Uint8Array => {
  let count = 0
  for (const byte of bytes) {
    if (byte === IAC) count++
  }
  if (count === 0) return bytes
  const out = new Uint8Array(bytes.length + count)
  let index = 0
  for (const byte of bytes) {
    out[index++] = byte
    if (byte === IAC) out[index++] = IAC
  }
  return out
}

export const escapeData = doubleIac

export const negotiation = (verb: Verb, option: number): Uint8Array =>
  Uint8Array.of(IAC, VERB_BYTE[verb], option)

export const subnegotiation = (payload: Uint8Array): Uint8Array => {
  const body = doubleIac(payload)
  const out = new Uint8Array(body.length + 4)
  out.set([IAC, SB])
  out.set(body, 2)
  out.set([IAC, SE], body.length + 2)
  return out
}
