import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import type * as FileSystem from 'effect/FileSystem'
import * as Layer from 'effect/Layer'
import * as Match from 'effect/Match'
import * as Queue from 'effect/Queue'
import * as Redacted from 'effect/Redacted'
import * as Ref from 'effect/Ref'
import * as Schedule from 'effect/Schedule'
import type * as Scope from 'effect/Scope'
import { createHash } from 'node:crypto'
import { mkdtemp, readdir, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import {
  type AgentMsg,
  assemble,
  assembled,
  Code,
  emptyAssembly,
  Epoch,
  type Frame,
  Mac,
  Md5,
  type Op,
  type OpFailure,
  type OpOutcome,
  opDataFrames,
  type RequestId,
} from '@cycling/protocol'
import * as BunFileSystem from '@effect/platform-bun/BunFileSystem'

import { AgentConfig } from '../src/AgentConfig.ts'
import * as Bridge from '../src/Bridge.ts'
import { BunTerminalPtyHost, PtyHost } from '../src/PtyHost.ts'
import * as RelayClient from '../src/RelayClient.ts'
import { Remote, layer as RemoteLayer } from '../src/Remote.ts'
import {
  fakeRelay,
  type FakeRelay,
  OPEN_SNAPSHOT,
} from './support/fakeRelay.ts'

const TOKEN = 'test-token-0123456789abcdef-0123456789'
const CODE = Code.make('7KQM4XP2')
const FAKE_MAC = Mac.make('020000a1b2c3')
const FLASH_BYTES = 1024 * 1024

type Services = Remote | PtyHost | AgentConfig | FileSystem.FileSystem

const md5 = (bytes: Uint8Array) =>
  Md5.make(createHash('md5').update(bytes).digest('hex'))

/**
 * Plays the visitor's page behind the fake relay: a 1 MiB flash in memory,
 * answering every op the agent sends, and answering a resent id again as the
 * page does. On the first op of its kind, `drops` closes the agent's socket
 * before the op runs, `losesAnswerOf` runs the op and closes the socket in
 * place of its answer, and `refuses` answers Failed with that reason.
 */
const fakePage = (
  relay: FakeRelay,
  options: {
    readonly drops?: Op['_tag']
    readonly losesAnswerOf?: Op['_tag']
    readonly refuses?: { readonly op: Op['_tag']; readonly reason: OpFailure }
  } = {},
) =>
  Effect.gen(function* () {
    const flash = new Uint8Array(FLASH_BYTES).map((_, i) => (i * 31) & 0xff)
    const ops = yield* Ref.make<ReadonlyArray<Op>>([])
    const uploads = new Map<RequestId, ReturnType<typeof emptyAssembly>>()
    const pending = new Map<RequestId, Extract<Op, { _tag: 'Write' }>>()
    const kinds = new Map<RequestId, Op['_tag']>()
    const answered = new Map<RequestId, OpOutcome>()
    const firsts = new Set<string>()
    let writes = 0

    const first = (option: string) =>
      !firsts.has(option) && !!firsts.add(option)

    const answer = (id: RequestId, outcome: OpOutcome) =>
      Effect.suspend(() => {
        answered.set(id, outcome)
        return kinds.get(id) === options.losesAnswerOf && first('losesAnswerOf')
          ? relay.closeAgent(1011)
          : relay.browser({ _tag: 'OpResult', id, outcome })
      })

    const finishWrite = (id: RequestId) =>
      Effect.gen(function* () {
        const op = pending.get(id)
        const upload = uploads.get(id)
        if (op === undefined || upload === undefined) return
        if (upload.received < op.region.size) return
        const bytes = assembled(upload)
        flash.set(bytes, op.region.address)
        writes += 1
        pending.delete(id)
        yield* answer(id, { _tag: 'Written', md5: md5(bytes) })
      })

    const onOp = (id: RequestId, op: Op) =>
      Effect.gen(function* () {
        yield* Ref.update(ops, all => [...all, op])
        kinds.set(id, op._tag)
        const already = answered.get(id)
        if (already !== undefined) {
          return yield* relay.browser({
            _tag: 'OpResult',
            id,
            outcome: already,
          })
        }
        if (op._tag === options.drops && first('drops')) {
          return yield* relay.closeAgent(1011)
        }
        if (op._tag === options.refuses?.op && first('refuses')) {
          return yield* relay.browser({
            _tag: 'OpResult',
            id,
            outcome: {
              _tag: 'Failed',
              reason: options.refuses.reason,
              detail: '',
            },
          })
        }
        return yield* Match.value(op).pipe(
          Match.tagsExhaustive({
            Enter: () =>
              answer(id, {
                _tag: 'Entered',
                chip: {
                  mac: FAKE_MAC,
                  secureBoot: false,
                  flashEncryption: false,
                },
              }),
            Read: ({ region: { address, size } }) =>
              Effect.gen(function* () {
                const bytes = flash.subarray(address, address + size)
                yield* relay.browser({
                  _tag: 'OpProgress',
                  id,
                  done: 0,
                  total: size,
                })
                for (const frame of opDataFrames(id, bytes)) {
                  yield* relay.sendBinary(frame)
                }
                yield* answer(id, { _tag: 'Read', md5: md5(bytes) })
              }),
            Write: write =>
              Effect.suspend(() => {
                pending.set(id, write)
                if (!uploads.has(id)) {
                  uploads.set(id, emptyAssembly(write.region.size))
                }
                return finishWrite(id)
              }),
            Digest: ({ region: { address, size } }) =>
              answer(id, {
                _tag: 'Digest',
                md5: md5(flash.subarray(address, address + size)),
              }),
            Reboot: () =>
              Effect.gen(function* () {
                yield* answer(id, { _tag: 'Rebooting' })
                yield* relay.browser({
                  _tag: 'PortChanged',
                  status: { epoch: Epoch.make(1), state: { _tag: 'Lost' } },
                })
                yield* Effect.sleep('50 millis')
                yield* relay.browser({
                  _tag: 'PortChanged',
                  status: {
                    epoch: Epoch.make(2),
                    state: { _tag: 'Open', baud: 115200 },
                  },
                })
                yield* relay.sendBytes(
                  new TextEncoder().encode(
                    'ESP-ROM:esp32s3-20210327\r\nboot ok\r\n',
                  ),
                )
              }),
            Release: () => answer(id, { _tag: 'Released' }),
          }),
        )
      })

    const onFrame = (frame: Frame<AgentMsg>) =>
      Effect.gen(function* () {
        if (frame._tag === 'Control')
          return yield* onOp(frame.message.id, frame.message.op)
        if (frame._tag !== 'OpData') return
        const upload =
          uploads.get(frame.id) ?? emptyAssembly(Number.MAX_SAFE_INTEGER)
        const next = assemble(upload, frame)
        if (next._tag === 'Some') uploads.set(frame.id, next.value)
        yield* finishWrite(frame.id)
      })

    yield* Effect.forkScoped(
      Effect.forever(Queue.take(relay.received).pipe(Effect.flatMap(onFrame))),
    )
    return {
      flash,
      ops: Ref.get(ops),
      writes: () => writes,
      /** Waits for the agent to give the lease back, as it does when its client goes. */
      released: Ref.get(ops).pipe(
        Effect.repeat({
          until: all => all.some(op => op._tag === 'Release'),
          schedule: Schedule.spaced('10 millis'),
        }),
        Effect.timeout('5 seconds'),
        Effect.orDie,
      ),
    }
  })

/** A devtools-shaped client: one JSON line out, one JSON line back. */
const doorClient = (path: string) =>
  Effect.acquireRelease(
    Effect.promise(async () => {
      const lines: Array<string> = []
      const waiters: Array<(line: string) => void> = []
      let buffer = ''
      const socket = await Bun.connect({
        unix: path,
        socket: {
          data: (_socket, data) => {
            buffer += data.toString()
            let newline = buffer.indexOf('\n')
            while (newline >= 0) {
              const line = buffer.slice(0, newline)
              buffer = buffer.slice(newline + 1)
              const waiter = waiters.shift()
              if (waiter === undefined) lines.push(line)
              else waiter(line)
              newline = buffer.indexOf('\n')
            }
          },
        },
      })
      const next = () =>
        new Promise<string>(resolve => {
          const line = lines.shift()
          if (line === undefined) waiters.push(resolve)
          else resolve(line)
        })
      return {
        socket,
        ask: (request: object): Effect.Effect<unknown> =>
          Effect.promise(() => {
            socket.write(`${JSON.stringify(request)}\n`)
            return next().then(line => JSON.parse(line))
          }).pipe(Effect.timeout('15 seconds'), Effect.orDie),
      }
    }),
    ({ socket }) => Effect.sync(() => socket.end()),
  )

const withBridge = <A, E>(
  program: (
    relay: FakeRelay,
    endpoints: Bridge.Endpoints,
    dir: string,
    notices: Queue.Queue<string>,
  ) => Effect.Effect<A, E, Services | Scope.Scope>,
) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.gen(function* () {
        const dir = yield* Effect.promise(() =>
          mkdtemp(join(tmpdir(), 'agent-door-')),
        )
        const relay = yield* fakeRelay({ snapshot: OPEN_SNAPSHOT })
        const layer = Layer.mergeAll(
          RemoteLayer.pipe(Layer.provide(RelayClient.layer)),
          BunTerminalPtyHost,
        ).pipe(
          Layer.provideMerge(
            Layer.mergeAll(
              Layer.succeed(
                AgentConfig,
                AgentConfig.of({
                  relay: relay.url,
                  token: Redacted.make(TOKEN),
                  dir,
                }),
              ),
              BunFileSystem.layer,
            ),
          ),
        )
        return yield* Effect.gen(function* () {
          const notices = yield* Queue.unbounded<string>()
          const port = yield* (yield* Remote).attach(CODE)
          const endpoints = yield* Bridge.open({
            port,
            dir,
            code: CODE,
            notice: text => Queue.offer(notices, text).pipe(Effect.asVoid),
          })
          return yield* program(relay, endpoints, dir, notices)
        }).pipe(Effect.provide(layer))
      }),
    ),
  )

describe('device socket', () => {
  test('enter, read, write, verify and reboot run as ops on the page, with files on this machine', () =>
    withBridge((relay, endpoints, dir, notices) =>
      Effect.gen(function* () {
        const page = yield* fakePage(relay)
        const door = yield* doorClient(endpoints.flasher)

        expect(yield* door.ask({ op: 'enter' })).toEqual({
          ok: { mac: FAKE_MAC, secure_boot: false, flash_encryption: false },
        })

        const readPath = join(dir, 'read.bin')
        const read = yield* door.ask({
          op: 'read',
          address: 0x8000,
          size: 0x48000,
          path: readPath,
        })
        const readBack = new Uint8Array(
          yield* Effect.promise(() => readFile(readPath)),
        )
        expect(readBack).toEqual(page.flash.slice(0x8000, 0x50000))
        expect(read).toEqual({ ok: { md5: md5(readBack) } })

        const image = new Uint8Array(0x21000).map((_, i) => (i * 7 + 3) & 0xff)
        const imagePath = join(dir, 'image.bin')
        yield* Effect.promise(() => writeFile(imagePath, image))
        expect(
          yield* door.ask({ op: 'write', address: 0x60000, path: imagePath }),
        ).toEqual({ ok: { md5: md5(image) } })
        expect(page.flash.slice(0x60000, 0x81000)).toEqual(image)
        expect(
          yield* door.ask({ op: 'verify', address: 0x60000, path: imagePath }),
        ).toEqual({ ok: { matched: true } })
        expect(
          yield* door.ask({ op: 'verify', address: 0x61000, path: imagePath }),
        ).toEqual({ ok: { matched: false } })

        const bootPath = join(dir, 'boot.log')
        expect(
          yield* door.ask({ op: 'reboot', seconds: 0.5, path: bootPath }),
        ).toEqual({ ok: { captured: true } })
        expect(
          yield* Effect.promise(() => readFile(bootPath, 'utf8')),
        ).toContain('ESP-ROM:esp32s3')

        const lines = yield* Queue.takeAll(notices)
        expect(lines.map(line => line.replace(/ in [\d.]+ s$/, ''))).toEqual([
          'flasher: enter',
          'flasher: read 288 KiB at 0x8000',
          'flasher: write at 0x60000',
          'flasher: verify at 0x60000',
          'flasher: verify at 0x61000',
          'flasher: reboot',
        ])
        expect(lines.join('\n')).not.toContain(FAKE_MAC)
        door.socket.end()
        yield* page.released
      }),
    ))

  test('closing the connection releases the lease, and a second connection meanwhile is busy', () =>
    withBridge((relay, endpoints) =>
      Effect.gen(function* () {
        const page = yield* fakePage(relay)
        const first = yield* doorClient(endpoints.flasher)
        yield* first.ask({ op: 'enter' })
        const second = yield* doorClient(endpoints.flasher)
        expect(yield* second.ask({ op: 'enter' })).toMatchObject({
          error: { reason: 'busy' },
        })
        first.socket.terminate()
        yield* page.released
        const ops = yield* page.ops
        expect(ops.map(op => op._tag)).toEqual(['Enter', 'Release'])
        expect(new Set(ops.map(op => op.lease)).size).toBe(1)
      }),
    ))

  test('a relay drop mid-read is redialed and the read resent under the same lease', () =>
    withBridge((relay, endpoints, dir) =>
      Effect.gen(function* () {
        const page = yield* fakePage(relay, { drops: 'Read' })
        const door = yield* doorClient(endpoints.flasher)
        yield* door.ask({ op: 'enter' })
        const path = join(dir, 'read.bin')
        const read = yield* door.ask({
          op: 'read',
          address: 0,
          size: 0x10000,
          path,
        })
        expect(read).toEqual({ ok: { md5: md5(page.flash.slice(0, 0x10000)) } })
        const ops = yield* page.ops
        expect(ops.map(op => op._tag)).toEqual(['Enter', 'Read', 'Read'])
        expect(new Set(ops.map(op => op.lease)).size).toBe(1)
        door.socket.end()
        yield* page.released
      }),
    ))

  test('an answer lost to a relay drop is asked for again under its id, and the page writes once', () =>
    withBridge((relay, endpoints, dir) =>
      Effect.gen(function* () {
        const page = yield* fakePage(relay, { losesAnswerOf: 'Write' })
        const door = yield* doorClient(endpoints.flasher)
        yield* door.ask({ op: 'enter' })
        const image = new Uint8Array(0x8000).map((_, i) => (i * 5) & 0xff)
        const imagePath = join(dir, 'image.bin')
        yield* Effect.promise(() => writeFile(imagePath, image))
        expect(
          yield* door.ask({ op: 'write', address: 0x20000, path: imagePath }),
        ).toEqual({ ok: { md5: md5(image) } })
        expect(page.writes()).toBe(1)
        door.socket.end()
        yield* page.released
      }),
    ))

  test('a write the page refuses transferGap is sent again and lands', () =>
    withBridge((relay, endpoints, dir) =>
      Effect.gen(function* () {
        const page = yield* fakePage(relay, {
          refuses: { op: 'Write', reason: 'transferGap' },
        })
        const door = yield* doorClient(endpoints.flasher)
        yield* door.ask({ op: 'enter' })
        const image = new Uint8Array(0x4000).fill(0x5a)
        const imagePath = join(dir, 'image.bin')
        yield* Effect.promise(() => writeFile(imagePath, image))
        expect(
          yield* door.ask({ op: 'write', address: 0x30000, path: imagePath }),
        ).toEqual({ ok: { md5: md5(image) } })
        expect(page.flash.slice(0x30000, 0x34000)).toEqual(image)
        door.socket.end()
        yield* page.released
      }),
    ))

  test('a request the socket does not understand, or past the flash, is refused without reaching the page', () =>
    withBridge((relay, endpoints) =>
      Effect.gen(function* () {
        const page = yield* fakePage(relay)
        const door = yield* doorClient(endpoints.flasher)
        expect(yield* door.ask({ op: 'erase' })).toMatchObject({
          error: { reason: 'badRequest' },
        })
        expect(
          yield* door.ask({
            op: 'read',
            address: 0,
            size: 16,
            path: 'relative',
          }),
        ).toMatchObject({ error: { reason: 'badRequest' } })
        expect(
          yield* door.ask({
            op: 'read',
            address: 0xff0000,
            size: 0x20000,
            path: '/tmp/x',
          }),
        ).toMatchObject({ error: { reason: 'badRequest' } })
        expect(yield* page.ops).toEqual([])
        door.socket.end()
        yield* page.released
      }),
    ))

  test('the closed bridge removes the device socket with the session dir', async () => {
    const dir = await withBridge((_relay, _endpoints, dir) =>
      Effect.succeed(dir),
    )
    expect(await readdir(dir)).toEqual([])
  })
})
