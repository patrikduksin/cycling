import { afterAll, describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Stream from 'effect/Stream'
import * as Socket from 'effect/socket/Socket'

import {
  type Binary,
  CloseCode,
  RequestId,
  encodeRelayMsg,
  KEEPALIVE,
  Lease,
  relaySocket,
  SUBPROTOCOL,
} from './index.ts'

interface Seen {
  readonly protocol: string
  readonly received: Array<string | Uint8Array>
}

const seen: Array<Seen> = []

/** Echoes the offered protocol named in the path, sends one Welcome-ish frame, one junk frame, bytes, then closes. */
const server = Bun.serve<Seen>({
  port: 0,
  hostname: '127.0.0.1',
  fetch(request, srv) {
    const url = new URL(request.url)
    const select = url.searchParams.get('select') ?? SUBPROTOCOL
    const state: Seen = { protocol: select, received: [] }
    seen.push(state)
    return srv.upgrade(request, {
      data: state,
      headers: { 'sec-websocket-protocol': select },
    })
      ? undefined
      : new Response('no', { status: 400 })
  },
  websocket: {
    open(ws) {
      ws.send(
        encodeRelayMsg({
          _tag: 'PeerPresence',
          role: 'browser',
          present: true,
        }),
      )
      ws.send('{"_tag":"Nonsense"}')
      ws.send(new Uint8Array([0, 1, 2, 3]))
      ws.send(KEEPALIVE.pong)
    },
    message(ws, message) {
      ws.data.received.push(
        typeof message === 'string' ? message : new Uint8Array(message),
      )
      if (typeof message !== 'string' && message[1] === 0xff) {
        ws.close(CloseCode.replaced, 'replaced')
      }
    },
  },
})
afterAll(() => server.stop(true))

const base = `ws://127.0.0.1:${server.port}/`
const consoleBytes = (bytes: ReadonlyArray<number>): Binary => ({
  _tag: 'Console',
  bytes: Uint8Array.from(bytes),
})

const run = <A, E>(
  effect: Effect.Effect<
    A,
    E,
    Socket.WebSocketConstructor | import('effect/Scope').Scope
  >,
) =>
  Effect.runPromise(
    effect.pipe(
      Effect.scoped,
      Effect.provide(Socket.layerWebSocketConstructorGlobal),
    ),
  )

describe('relaySocket', () => {
  test('decodes frames in order, drops junk, ends with the close code', async () => {
    const events = await run(
      Effect.gen(function* () {
        const socket = yield* relaySocket({
          role: 'agent',
          url: base,
          protocols: [SUBPROTOCOL, 'auth.x'],
        })
        expect(
          yield* socket.send({
            _tag: 'Op',
            id: RequestId.make(1),
            op: { _tag: 'Release', lease: Lease.make('0123456789abcdef') },
          }),
        ).toBe(true)
        expect(yield* socket.send(consoleBytes([9]))).toBe(true)
        expect(yield* socket.send(consoleBytes([0xff]))).toBe(true)
        const events = yield* Stream.runCollect(socket.events)
        expect(yield* socket.send(consoleBytes([9]))).toBe(false)
        return events
      }),
    )
    expect(events.map(event => event._tag)).toEqual([
      'Control',
      'Console',
      'Pong',
      'Closed',
    ])
    expect(events.at(-1)).toEqual({ _tag: 'Closed', code: CloseCode.replaced })
  })

  test('refuses a relay that picked another subprotocol', async () => {
    const exit = await Effect.runPromiseExit(
      relaySocket({
        role: 'browser',
        url: `${base}?select=auth.x`,
        protocols: [SUBPROTOCOL, 'auth.x'],
      }).pipe(
        Effect.scoped,
        Effect.provide(Socket.layerWebSocketConstructorGlobal),
      ),
    )
    expect(exit._tag).toBe('Failure')
    expect(JSON.stringify(exit)).toContain('subprotocol')
  })

  test('an unreachable relay is a dial error, not a hang', async () => {
    const exit = await Effect.runPromiseExit(
      relaySocket({
        role: 'browser',
        url: 'ws://127.0.0.1:1/',
        protocols: [SUBPROTOCOL],
      }).pipe(
        Effect.scoped,
        Effect.provide(Socket.layerWebSocketConstructorGlobal),
      ),
    )
    expect(JSON.stringify(exit)).toContain('unreachable')
  })
})
