/**
 * Headers the Worker sets on the request it forwards to a Session. Only the
 * Worker can reach the Durable Object namespace, so the object trusts them.
 */
export const ROLE_HEADER = 'x-cycling-role'
export const OWNER_HEADER = 'x-cycling-owner'
