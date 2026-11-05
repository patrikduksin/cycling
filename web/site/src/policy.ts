import * as Option from 'effect/Option'

import { type Code, sessionPath } from '@cycling/protocol'

import type { Environment, Missing } from './model.ts'

export const REACQUIRE = { timeoutMs: 60_000, pollMs: 250 } as const

/** How long the page keeps an unattended hold for an agent that went away, so it can come back to the same lease. */
export const AGENT_GRACE_MS = 30_000

/**
 * How long a hold may sit with nothing queued while the agent is here. A
 * device task asks its next op within seconds; this ends a hold whose
 * Release was lost on the way, and gives the device back to its app.
 */
export const IDLE_HOLD_MS = 120_000

/** The running op and one behind it; the agent sends one at a time, so a third is `busy`. */
export const OP_QUEUE = 2

/** Lines in the visitor's activity list. */
export const ACTIVITY_ROWS = 20

/** Relay bytes waiting for the port, in frames; more than this while a write is stuck are dropped. */
export const WRITE_QUEUE_FRAMES = 32

/** Why the page cannot work here, checked in the order a visitor can fix them. */
export const detectMissing = (
  environment: Environment,
): Option.Option<Missing> => {
  if (!environment.isSecureContext) {
    return Option.some('secureContext')
  }
  if (!environment.hasWebSerial) {
    return Option.some('webSerial')
  }
  if (!environment.relayConfigured) {
    return Option.some('relayUrl')
  }
  return Option.none()
}

const SOCKET_PROTOCOL: Readonly<Record<string, string>> = {
  'http:': 'ws:',
  'https:': 'wss:',
  'ws:': 'ws:',
  'wss:': 'wss:',
}

/** `VITE_RELAY_URL` as a WebSocket origin; None when unset or not a URL. */
export const parseRelayBase = (
  raw: string | undefined,
): Option.Option<string> =>
  Option.fromNullishOr(raw).pipe(
    Option.map(value => value.trim()),
    Option.filter(value => URL.canParse(value)),
    Option.flatMap(value => {
      const url = new URL(value)
      return Option.fromNullishOr(SOCKET_PROTOCOL[url.protocol]).pipe(
        Option.map(protocol => `${protocol}//${url.host}`),
      )
    }),
  )

export const relayEndpoint = (base: string, code: Code): string =>
  `${base}${sessionPath('browser', code)}`
