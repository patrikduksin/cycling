import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Fiber from 'effect/Fiber'
import * as Layer from 'effect/Layer'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as Socket from 'effect/socket/Socket'
import { TestClock } from 'effect/testing'

import {
  CloseCode,
  KEEPALIVE,
  SUBPROTOCOL,
  encodeRelayMsg,
} from '@cycling/protocol'

import { session } from '../test/fixtures.ts'
import { browserRelay } from './relay.ts'

type Listener = (event: Socket.WebSocketEvent) => void
type EventName = 'open' | 'message' | 'error' | 'close'

/** A browser WebSocket as `effect/socket` sees it; `accept` echoes `cycling.v2` like the relay. */
class FakeWebSocket implements Socket.WebSocketLike {
  readyState = 0
  protocol = ''
  binaryType = 'blob'
  readonly sent: Array<string | Uint8Array> = []
  readonly closes: Array<number | undefined> = []
  private readonly listeners = new Map<EventName, Set<Listener>>()

  constructor(
    readonly url: string,
    readonly protocols: ReadonlyArray<string>,
  ) {}

  addEventListener(type: EventName, listener: Listener): void {
    const set = this.listeners.get(type) ?? new Set()
    set.add(listener)
    this.listeners.set(type, set)
  }

  removeEventListener(type: EventName, listener: Listener): void {
    this.listeners.get(type)?.delete(listener)
  }

  send(data: string | Uint8Array): void {
    this.sent.push(data)
  }

  close(code?: number): void {
    this.closes.push(code)
  }

  accept(): void {
    this.protocol = SUBPROTOCOL
    this.readyState = 1
    this.emit('open', {})
  }

  receive(data: string | ArrayBuffer): void {
    this.emit('message', { data })
  }

  drop(code: number): void {
    this.readyState = 3
    this.emit('close', { code })
  }

  private emit(type: EventName, event: Socket.WebSocketEvent): void {
    this.listeners.get(type)?.forEach(listener => listener(event))
  }
}

const harness = () => {
  const sockets: Array<FakeWebSocket> = []
  const webSocket = Layer.succeed(Socket.WebSocketConstructor)((
    url,
    options,
  ) => {
    const created = new FakeWebSocket(
      url,
      Array.isArray(options) ? options : [],
    )
    sockets.push(created)
    return created
  })
  const dial = browserRelay({ base: 'wss://relay.test', webSocket }).dial(
    session,
  )
  /** Starts a dial and has the relay accept it. */
  const open = Effect.gen(function* () {
    const fiber = yield* Effect.forkChild(dial)
    yield* settle
    const socket = sockets.at(-1)
    if (socket === undefined) {
      return yield* Effect.die('no socket was created')
    }
    socket.accept()
    return { link: yield* Fiber.join(fiber), socket }
  })
  return { open }
}

const settle = Effect.replicateEffect(Effect.yieldNow, 20, { discard: true })

const run = <A, E>(program: Effect.Effect<A, E, Scope.Scope>): Promise<A> =>
  Effect.runPromise(
    program.pipe(Effect.scoped, Effect.provide(TestClock.layer())),
  )

const welcome = encodeRelayMsg({
  _tag: 'Welcome',
  role: 'browser',
  snapshot: {
    device: null,
    port: null,
    expiresAt: 1,
    browserPresent: true,
    agentPresent: false,
  },
})

describe('browser relay adapter', () => {
  test('dials the versioned browser path with the owner key as a subprotocol', () =>
    run(
      Effect.gen(function* () {
        const { socket } = yield* harness().open

        expect(socket.url).toBe('wss://relay.test/v2/browser/7KQM4XP2')
        expect(socket.protocols).toEqual([
          'cycling.v2',
          `own.${session.ownerKey}`,
        ])
        expect(socket.binaryType).toBe('arraybuffer')
      }),
    ))

  test('keeps frames that arrive before anyone reads them, decoded in order, ending with the close code', () =>
    run(
      Effect.gen(function* () {
        const { link, socket } = yield* harness().open
        socket.receive(welcome)
        socket.receive(Uint8Array.of(0, 1, 2, 3).buffer)
        socket.receive('pong')
        socket.receive('{"_tag":"Welcome","extra":1}')
        socket.drop(CloseCode.expired)

        const events = yield* Stream.runCollect(link.events)
        expect(events.map(event => event._tag)).toEqual([
          'Control',
          'Console',
          'Pong',
          'Closed',
        ])
        expect(events.at(-1)).toEqual({ _tag: 'Closed', code: 4408 })
      }),
    ))

  test('sends console bytes as channel-0 binary and control as JSON, and reports a closed socket', () =>
    run(
      Effect.gen(function* () {
        const { link, socket } = yield* harness().open

        expect(
          yield* link.send({ _tag: 'Console', bytes: Uint8Array.of(7, 8) }),
        ).toBe(true)
        expect(yield* link.send({ _tag: 'Bye' })).toBe(true)
        const [binary, text] = socket.sent
        expect(
          typeof binary === 'string' ? binary : Array.from(binary ?? []),
        ).toEqual([0, 7, 8])
        expect(text).toBe('{"_tag":"Bye"}')

        socket.drop(CloseCode.visitorDisconnected)
        expect(
          yield* link.send({ _tag: 'Console', bytes: Uint8Array.of(9) }),
        ).toBe(false)
        expect(socket.sent).toHaveLength(2)
      }),
    ))

  test('pings once per keepalive interval', () =>
    run(
      Effect.gen(function* () {
        const { socket } = yield* harness().open

        yield* TestClock.adjust(KEEPALIVE.intervalMs)
        yield* settle
        yield* TestClock.adjust(KEEPALIVE.intervalMs)
        yield* settle

        expect(
          socket.sent.filter(frame => frame === KEEPALIVE.ping),
        ).toHaveLength(2)
      }),
    ))

  test('closing the scope closes the socket', () =>
    run(
      Effect.gen(function* () {
        const { socket } = yield* harness().open.pipe(Effect.scoped)

        expect(socket.closes).toContain(1000)
      }),
    ))
})
