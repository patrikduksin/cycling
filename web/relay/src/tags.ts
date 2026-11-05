import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import { Role } from '@cycling/protocol'

/**
 * What a Durable Object knows about one of its sockets, kept in the socket's
 * tags so it survives hibernation. A refused socket exists only to deliver
 * its close frame; it is never a peer.
 */
export type SocketTag =
  | { readonly _tag: 'Live'; readonly role: Role; readonly id: string }
  | { readonly _tag: 'Refused' }

const ID_PREFIX = 'id:'
const REFUSED = 'refused'

const decodeRole = Schema.decodeUnknownOption(Role)

/**
 * The tags to accept a socket with. The role tag comes first and is the bare
 * role name, so `getWebSockets(role)` finds every live socket of that role.
 */
export const encodeTag = (tag: SocketTag): Array<string> =>
  tag._tag === 'Refused' ? [REFUSED] : [tag.role, `${ID_PREFIX}${tag.id}`]

/**
 * The inverse of `encodeTag`, whatever order the runtime returns the tags in;
 * None for tags this relay did not write.
 */
export const parseTag = (
  tags: ReadonlyArray<string>,
): Option.Option<SocketTag> => {
  const ids = tags.filter(tag => tag.startsWith(ID_PREFIX))
  const [named, ...extra] = tags.filter(tag => !tag.startsWith(ID_PREFIX))
  const [id, ...moreIds] = ids
  if (named === undefined || extra.length > 0 || moreIds.length > 0) {
    return Option.none()
  }
  if (id === undefined) {
    return named === REFUSED ? Option.some({ _tag: 'Refused' }) : Option.none()
  }
  return decodeRole(named).pipe(
    Option.map((role): SocketTag => ({
      _tag: 'Live',
      role,
      id: id.slice(ID_PREFIX.length),
    })),
  )
}
