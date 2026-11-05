import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Ref from 'effect/Ref'
import * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import * as BunSocket from '@effect/platform-bun/BunSocket'

import { initialParser, parse } from '../../src/rfc2217/Telnet.ts'
import { connection } from '../../src/sockets.ts'

const IAC = 0xff
const SB = 0xfa
const SE = 0xf0
const COM_PORT_OPTION = 44
const SET_CONTROL = 5
const PURGE_DATA = 12

/** The SET_CONTROL value for one fixture token: D1 is DTR on (8), R0 is RTS off (12). */
const CONTROL_VALUE: Readonly<Record<string, number>> = {
  D1: 8,
  D0: 9,
  R1: 11,
  R0: 12,
}

/** A bare TCP client speaking just enough RFC 2217 to replay recorded control tokens. */
export const rfcClient = (url: string) =>
  Effect.gen(function* () {
    const port = Number(new URL(url.replace('rfc2217:', 'http:')).port)
    const received = yield* Ref.make<ReadonlyArray<number>>([])
    const closed = yield* Ref.make(false)
    const line = yield* Scope.fork(yield* Effect.scope)
    const client = yield* connection(
      yield* BunSocket.makeNet({ host: '127.0.0.1', port }),
    ).pipe(Scope.provide(line), Effect.orDie)
    yield* Effect.forkScoped(
      client.incoming.pipe(
        Stream.runForEach(bytes =>
          Ref.update(received, all => [...all, ...bytes]),
        ),
        Effect.ensuring(Ref.set(closed, true)),
      ),
    )
    const send = (bytes: ReadonlyArray<number>) =>
      client.write(Uint8Array.from(bytes))
    return {
      send,
      replay: (tokens: string) =>
        Effect.forEach(
          tokens.split(/\s+/).filter(token => token.length > 0),
          token =>
            send([
              IAC,
              SB,
              COM_PORT_OPTION,
              ...(token === 'P'
                ? [PURGE_DATA, 3]
                : [SET_CONTROL, CONTROL_VALUE[token] ?? 0]),
              IAC,
              SE,
            ]),
        ),
      /** Device bytes the client has seen so far, telnet removed. */
      data: Ref.get(received).pipe(
        Effect.map(all =>
          parse(initialParser, Uint8Array.from(all)).items.flatMap(item =>
            item._tag === 'Data' ? [...item.bytes] : [],
          ),
        ),
      ),
      closed: Ref.get(closed),
      end: Scope.close(line, Exit.void),
    }
  })
