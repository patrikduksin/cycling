import * as Schema from 'effect/Schema'

import { Device, PortStatus } from '@cycling/protocol'

const fields = {
  /** Hex SHA-256 of the owner key; the key itself never reaches the Durable Object. */
  ownerKeyHash: Schema.String.check(Schema.isPattern(/^[0-9a-f]{64}$/)),
  createdAt: Schema.Int,
  expiresAt: Schema.Int,
  /** When the last browser socket went, or null while a browser is attached. */
  browserGoneAt: Schema.NullOr(Schema.Int),
  device: Schema.NullOr(Device),
  port: Schema.NullOr(PortStatus),
}

/**
 * The only durable state of a session. Presence is never stored: it is the
 * count of live sockets per role. An ended session has no record, so "no
 * record" is the unclaimed phase.
 */
export const SessionRecord = Schema.Union([
  Schema.TaggedStruct('Open', fields),
  Schema.TaggedStruct('Paired', fields),
])
export type SessionRecord = typeof SessionRecord.Type
