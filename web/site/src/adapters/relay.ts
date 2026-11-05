import * as Effect from 'effect/Effect'
import type * as Layer from 'effect/Layer'
import type * as Socket from 'effect/socket/Socket'

import { browserProtocols, relaySocket } from '@cycling/protocol'

import type { RelayAdapter } from '../links.ts'
import { relayEndpoint } from '../policy.ts'

/**
 * The browser's relay socket: the session's versioned path, the owner key as
 * a subprotocol. `webSocket` is `Socket.layerWebSocketConstructorGlobal` in
 * the page and a fake in tests.
 */
export const browserRelay = (options: {
  readonly base: string
  readonly webSocket: Layer.Layer<Socket.WebSocketConstructor>
}): RelayAdapter => ({
  dial: session =>
    relaySocket({
      role: 'browser',
      url: relayEndpoint(options.base, session.code),
      protocols: browserProtocols(session.ownerKey),
    }).pipe(Effect.provide(options.webSocket)),
})
