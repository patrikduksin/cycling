import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Fiber from 'effect/Fiber'
import * as Layer from 'effect/Layer'
import * as Option from 'effect/Option'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import { lstat, mkdtemp, readlink } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import * as BunFileSystem from '@effect/platform-bun/BunFileSystem'

import {
  BunTerminalPtyHost,
  PTY_BACKLOG_CHUNKS,
  PtyHost,
} from '../src/PtyHost.ts'
import { ptyClient } from './support/ptyClient.ts'

const ALL_BYTES = Uint8Array.from({ length: 256 }, (_, i) => i)

const concat = (a: Uint8Array, b: Uint8Array) => {
  const out = new Uint8Array(a.length + b.length)
  out.set(a)
  out.set(b, a.length)
  return out
}

const takeBytes = (stream: Stream.Stream<Uint8Array>, count: number) =>
  stream.pipe(
    Stream.scan(() => new Uint8Array(0), concat),
    Stream.filter(bytes => bytes.length >= count),
    Stream.runHead,
    Effect.map(Option.getOrElse(() => new Uint8Array(0))),
    Effect.timeout('5 seconds'),
  )

const run = <A, E>(program: Effect.Effect<A, E, PtyHost | Scope.Scope>) =>
  Effect.runPromise(
    Effect.scoped(program).pipe(
      Effect.provide(
        BunTerminalPtyHost.pipe(Layer.provide(BunFileSystem.layer)),
      ),
    ),
  )

const openPty = Effect.gen(function* () {
  const dir = yield* Effect.promise(() => mkdtemp(join(tmpdir(), 'agent-pty-')))
  const host = yield* PtyHost
  return yield* host.open(join(dir, 'port'))
})

const text = (value: string) => new TextEncoder().encode(value)

describe('Bun.Terminal pty (R5)', () => {
  test('all 256 byte values cross 8-bit clean in both directions', () =>
    run(
      Effect.gen(function* () {
        const pty = yield* openPty
        const client = yield* ptyClient(pty.path)
        yield* client.open
        yield* pty.write(ALL_BYTES)
        expect([...(yield* client.read(256, 3))]).toEqual([...ALL_BYTES])
        const fromTool = yield* Effect.forkChild(takeBytes(pty.input, 256))
        yield* client.write(ALL_BYTES)
        expect([...(yield* Fiber.join(fromTool))]).toEqual([...ALL_BYTES])
      }),
    ))

  test('with no reader the bridge never blocks, counts drops, and a later reader gets fresh bytes', () =>
    run(
      Effect.gen(function* () {
        const pty = yield* openPty
        const chunk = new Uint8Array(4096).fill(0x78)
        const chunks = 384
        const started = performance.now()
        let accepted = 0
        for (let i = 0; i < chunks; i++) {
          if (yield* pty.write(chunk)) accepted++
        }
        const elapsed = performance.now() - started
        const dropped = yield* pty.dropped
        expect(elapsed).toBeLessThan(500)
        expect(dropped).toBeGreaterThan(1024 * 1024)
        expect(dropped).toBe((chunks - accepted) * chunk.length)
        const client = yield* ptyClient(pty.path)
        yield* client.open
        const backlog = yield* client.drain(1)
        expect(backlog).toBeLessThan(
          (PTY_BACKLOG_CHUNKS + 1) * chunk.length + 64 * 1024,
        )
        const fresh = text('FRESH-AFTER-STALL\n')
        yield* pty.write(fresh)
        expect(
          new TextDecoder().decode(yield* client.read(fresh.length, 3)),
        ).toBe('FRESH-AFTER-STALL\n')
        console.log(
          `R5 backlog: wrote ${chunks * chunk.length} B in ${elapsed.toFixed(1)} ms with no reader, dropped ${dropped} B, next reader drained ${backlog} B of backlog`,
        )
      }),
    ))

  test('the pty survives five open/close cycles by a devtools-shaped client', () =>
    run(
      Effect.gen(function* () {
        const pty = yield* openPty
        const target = yield* Effect.promise(() => readlink(pty.path))
        for (let cycle = 0; cycle < 5; cycle++) {
          const client = yield* ptyClient(pty.path)
          yield* client.open
          const request = text(`CMD ${cycle} INFO\n`)
          const seen = yield* Effect.forkChild(
            takeBytes(pty.input, request.length),
          )
          yield* client.write(request)
          expect([...(yield* Fiber.join(seen))]).toEqual([...request])
          const reply = text(`{"type":"reply","id":${cycle}}\n`)
          yield* pty.write(reply)
          expect([...(yield* client.read(reply.length, 3))]).toEqual([...reply])
          yield* client.close
        }
        expect(yield* Effect.promise(() => readlink(pty.path))).toBe(target)
      }),
    ))

  test('closing the scope removes the symlink', async () => {
    const path = await run(Effect.map(openPty, pty => pty.path))
    expect(
      await lstat(path).then(
        () => 'exists',
        () => 'gone',
      ),
    ).toBe('gone')
  })
})
