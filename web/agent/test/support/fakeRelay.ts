import type { ServerWebSocket } from 'bun'
import * as Effect from 'effect/Effect'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'

import {
  type AgentMsg,
  type Binary,
  type BrowserMsg,
  decodeFromAgent,
  encodeBinary,
  encodeBrowserMsg,
  encodeRelayMsg,
  type Frame,
  KEEPALIVE,
  type RelayMsg,
  type Snapshot,
  SUBPROTOCOL,
} from '@cycling/protocol'

import { C606, EPOCH_1, BAUD_115200 } from './fakePort.ts'

export const OPEN_SNAPSHOT: Snapshot = {
  device: C606,
  port: { epoch: EPOCH_1, state: { _tag: 'Open', baud: BAUD_115200 } },
  expiresAt: 1_900_000_000_000,
  browserPresent: true,
  agentPresent: true,
}

/** What the relay does with the nth agent connection: welcome it, or close it with a code. */
export type Admission = (connection: number) => 'welcome' | number

/**
 * Plays the relay and the visitor's browser on loopback: welcomes the agent
 * with a snapshot, answers `ping`, records every frame the agent sends, and
 * lets a test send console bytes, op data and browser messages.
 */
export const fakeRelay = (options: {
  readonly snapshot: Snapshot
  readonly admit?: Admission
  /** What the 101 selects; a real relay always echoes cycling.v2. */
  readonly subprotocol?: string
}) =>
  Effect.gen(function* () {
    const received = yield* Queue.unbounded<Frame<AgentMsg>>()
    const offered = yield* Ref.make<ReadonlyArray<string | null>>([])
    const paths = yield* Ref.make<ReadonlyArray<string>>([])
    const current = yield* Ref.make<ServerWebSocket<unknown> | null>(null)
    const connections = yield* Ref.make(0)
    const runSync = Effect.runSyncWith(yield* Effect.context<never>())
    const admit = options.admit ?? (() => 'welcome')

    const server = yield* Effect.acquireRelease(
      Effect.sync(() =>
        Bun.serve({
          hostname: '127.0.0.1',
          port: 0,
          fetch: (request, server) => {
            runSync(
              Effect.all([
                Ref.update(offered, all => [
                  ...all,
                  request.headers.get('sec-websocket-protocol'),
                ]),
                Ref.update(paths, all => [
                  ...all,
                  new URL(request.url).pathname,
                ]),
              ]),
            )
            return server.upgrade(request, {
              headers: {
                'Sec-WebSocket-Protocol': options.subprotocol ?? SUBPROTOCOL,
              },
              data: undefined,
            })
              ? undefined
              : new Response('upgrade required', { status: 426 })
          },
          websocket: {
            open: ws => {
              const n = runSync(Ref.updateAndGet(connections, k => k + 1))
              const verdict = admit(n)
              if (verdict !== 'welcome') {
                ws.close(verdict, 'refused')
                return
              }
              runSync(Ref.set(current, ws))
              ws.send(
                encodeRelayMsg({
                  _tag: 'Welcome',
                  role: 'agent',
                  snapshot: options.snapshot,
                }),
              )
            },
            message: (ws, message) => {
              if (message === KEEPALIVE.ping) {
                ws.send(KEEPALIVE.pong)
                return
              }
              const frame = runSync(Effect.result(decodeFromAgent(message)))
              if (frame._tag === 'Success') {
                Queue.offerUnsafe(received, frame.success)
              }
            },
            close: ws => {
              runSync(Ref.update(current, open => (open === ws ? null : open)))
            },
          },
        }),
      ),
      server => Effect.promise(() => server.stop(true)),
    )

    const withAgent = (use: (ws: ServerWebSocket<unknown>) => void) =>
      Ref.get(current).pipe(
        Effect.flatMap(ws =>
          ws === null
            ? Effect.die('no agent is connected to the fake relay')
            : Effect.sync(() => use(ws)),
        ),
      )

    return {
      url: `ws://127.0.0.1:${server.port}`,
      received,
      offered: Ref.get(offered),
      paths: Ref.get(paths),
      connections: Ref.get(connections),
      sendBytes: (bytes: Uint8Array) =>
        withAgent(ws => ws.send(encodeBinary({ _tag: 'Console', bytes }))),
      sendBinary: (binary: Binary) =>
        withAgent(ws => ws.send(encodeBinary(binary))),
      browser: (message: BrowserMsg) =>
        withAgent(ws => ws.send(encodeBrowserMsg(message))),
      relay: (message: RelayMsg) =>
        withAgent(ws => ws.send(encodeRelayMsg(message))),
      closeAgent: (code: number) => withAgent(ws => ws.close(code, 'test')),
    }
  })

export type FakeRelay = Effect.Success<ReturnType<typeof fakeRelay>>
