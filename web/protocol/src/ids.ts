import * as Schema from 'effect/Schema'

/** Wire major version. It is the URL prefix and the `cycling.v<N>` subprotocol; any wire change bumps it. */
export const PROTOCOL_VERSION = 3

const CROCKFORD = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'

/**
 * 8 Crockford base32 characters, 40 bits, minted by the browser. A code
 * addresses one session and pairs two humans; it authorizes nothing (the
 * owner key and the agent token do).
 */
export const Code = Schema.String.check(
  Schema.isPattern(/^[0-9A-HJKMNP-TV-Z]{8}$/),
).pipe(Schema.brand('SessionCode'))
export type Code = typeof Code.Type

/** 128 bits, base64url without padding. Held only by the browser; the relay stores its SHA-256. */
export const OwnerKey = Schema.String.check(
  Schema.isPattern(/^[A-Za-z0-9_-]{22}$/),
).pipe(Schema.brand('OwnerKey'))
export type OwnerKey = typeof OwnerKey.Type

/** Increments each time the browser opens the port, so a re-enumeration is visible as a new epoch. */
export const Epoch = Schema.Int.pipe(Schema.brand('Epoch'))
export type Epoch = typeof Epoch.Type

/** Correlates an agent request with the browser's result; unique per agent connection. */
export const RequestId = Schema.Int.pipe(Schema.brand('RequestId'))
export type RequestId = typeof RequestId.Type

export const UsbId = Schema.Int.check(
  Schema.isBetween({ minimum: 0, maximum: 0xffff }),
)
export type UsbId = typeof UsbId.Type

export const Baud = Schema.Int.check(
  Schema.isBetween({ minimum: 300, maximum: 4_000_000 }),
)
export type Baud = typeof Baud.Type

export const Role = Schema.Literals(['browser', 'agent'])
export type Role = typeof Role.Type

const decodeCode = Schema.decodeUnknownOption(Code)

export const mintCode = (random: Uint8Array): Code => {
  let bits = 0n
  for (const byte of random.subarray(0, 5)) {
    bits = (bits << 8n) | BigInt(byte)
  }
  let text = ''
  for (let shift = 35n; shift >= 0n; shift -= 5n) {
    text += CROCKFORD.charAt(Number((bits >> shift) & 31n))
  }
  return Schema.decodeSync(Code)(text)
}

export const mintOwnerKey = (random: Uint8Array): OwnerKey => {
  const binary = String.fromCharCode(...random.subarray(0, 16))
  const text = btoa(binary)
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replace(/=+$/, '')
  return Schema.decodeSync(OwnerKey)(text)
}

/**
 * What a human types or pastes: any case, dashes or spaces, and the Crockford
 * look-alikes (I and L read as 1, O as 0).
 */
export const parseCode = (input: string) =>
  decodeCode(
    input
      .toUpperCase()
      .replace(/[\s-]/g, '')
      .replace(/[IL]/g, '1')
      .replace(/O/g, '0'),
  )

export const formatCode = (code: Code): string =>
  `${code.slice(0, 4)}-${code.slice(4)}`
