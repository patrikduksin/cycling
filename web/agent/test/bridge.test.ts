import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import type * as FileSystem from 'effect/FileSystem'
import * as Layer from 'effect/Layer'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Redacted from 'effect/Redacted'
import * as Schedule from 'effect/Schedule'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as SubscriptionRef from 'effect/SubscriptionRef'
import { mkdtemp, readdir } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import {
  type AgentMsg,
  Code,
  Epoch,
  type Frame,
  type ResetKind,
} from '@cycling/protocol'
import * as BunFileSystem from '@effect/platform-bun/BunFileSystem'

import { AgentConfig } from '../src/AgentConfig.ts'
import * as Bridge from '../src/Bridge.ts'
import { requestReset } from '../src/Control.ts'
import { BunTerminalPtyHost, PtyHost } from '../src/PtyHost.ts'
import * as RelayClient from '../src/RelayClient.ts'
import { Remote, layer as RemoteLayer } from '../src/Remote.ts'
import { isPortOpen, type RemoteStatus } from '../src/RemotePort.ts'
import { C606_IDENTITY } from './support/fakePort.ts'
import {
  type Admission,
  fakeRelay,
  type FakeRelay,
  OPEN_SNAPSHOT,
} from './support/fakeRelay.ts'
import { ptyClient } from './support/ptyClient.ts'
import { rfcClient } from './support/rfcClient.ts'

const TOKEN = 'test-token-0123456789abcdef-0123456789'
const CODE = Code.make('7KQM4XP2')
const text = (value: string) => new TextEncoder().encode(value)

type Services = Remote | PtyHost | AgentConfig | FileSystem.FileSystem

const agentLayer = (relay: FakeRelay, dir: string) =>
  Layer.mergeAll(
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

const withAgent = <A, E>(
  program: (
    relay: FakeRelay,
    dir: string,
  ) => Effect.Effect<A, E, Services | Scope.Scope>,
  relayOptions: {
    readonly admit?: Admission
    readonly subprotocol?: string
  } = {},
) =>
  Effect.runPromise(
    Effect.scoped(
      Effect.gen(function* () {
        const dir = yield* Effect.promise(() =>
          mkdtemp(join(tmpdir(), 'agent-bridge-')),
        )
        const relay = yield* fakeRelay({
          snapshot: OPEN_SNAPSHOT,
          ...relayOptions,
        })
        return yield* program(relay, dir).pipe(
          Effect.provide(agentLayer(relay, dir)),
        )
      }),
    ),
  )

const eventually = <A>(
  probe: Effect.Effect<A>,
  ok: (value: A) => boolean,
): Effect.Effect<A> =>
  Effect.repeat(probe, {
    until: ok,
    schedule: Schedule.spaced('10 millis'),
  }).pipe(
    Effect.timeoutOrElse({
      duration: '5 seconds',
      orElse: () => probe.pipe(Effect.flatMap(last => Effect.die(last))),
    }),
  )

const statusUntil = (
  status: SubscriptionRef.SubscriptionRef<RemoteStatus>,
  ok: (status: RemoteStatus) => boolean,
) => eventually(SubscriptionRef.get(status), ok)

const nextControl = (relay: FakeRelay) =>
  Queue.take(relay.received).pipe(
    Effect.repeat({
      until: (frame: Frame<AgentMsg>) => frame._tag === 'Control',
    }),
    Effect.flatMap(frame =>
      frame._tag === 'Control'
        ? Effect.succeed(frame.message)
        : Effect.die(frame),
    ),
    Effect.timeout('5 seconds'),
  )

const nextBytes = (relay: FakeRelay) =>
  Queue.take(relay.received).pipe(
    Effect.flatMap(frame =>
      frame._tag === 'Bytes'
        ? Effect.succeed([...frame.bytes])
        : Effect.die(frame),
    ),
    Effect.timeout('5 seconds'),
  )

const attachAndOpen = Effect.gen(function* () {
  const port = yield* (yield* Remote).attach(CODE)
  const config = yield* AgentConfig
  const endpoints = yield* Bridge.open({
    port,
    identity: C606_IDENTITY,
    dir: config.dir,
    code: CODE,
  })
  return { port, endpoints }
})

describe('agent against a fake relay', () => {
  test('R2: the agent offers cycling.v1 and its token as subprotocols on /v1/agent/<code>', () =>
    withAgent(relay =>
      Effect.gen(function* () {
        yield* (yield* Remote).attach(CODE)
        expect(yield* relay.offered).toEqual([`cycling.v1, auth.${TOKEN}`])
        expect(yield* relay.paths).toEqual([`/v1/agent/${CODE}`])
      }),
    ))

  test('device bytes reach the pty client and pty writes reach the browser', () =>
    withAgent(relay =>
      Effect.gen(function* () {
        const { endpoints } = yield* attachAndOpen
        const client = yield* ptyClient(endpoints.pty)
        yield* client.open
        const fromDevice = text('{"type":"log","boot":1}\n')
        yield* relay.sendBytes(fromDevice)
        expect([...(yield* client.read(fromDevice.length, 3))]).toEqual([
          ...fromDevice,
        ])
        const command = text('CMD 7 INFO\n')
        yield* client.write(command)
        expect(yield* nextBytes(relay)).toEqual([...command])
      }),
    ))

  test('the RFC 2217 client takes the door and gives it back to the pty', () =>
    withAgent(relay =>
      Effect.gen(function* () {
        const { endpoints } = yield* attachAndOpen
        const pty = yield* ptyClient(endpoints.pty)
        yield* pty.open
        const esptool = yield* rfcClient(endpoints.rfc2217)
        yield* eventually(
          SubscriptionRef.get(endpoints.doors),
          doors => doors.active === 'rfc2217',
        )
        const slip = Uint8Array.of(0xc0, 0x01, 0xff, 0xc0)
        yield* relay.sendBytes(slip)
        expect(
          yield* eventually(esptool.data, data => data.length >= slip.length),
        ).toEqual([...slip])
        expect([...(yield* pty.read(1, 0.3))]).toEqual([])
        yield* esptool.send([0x07, 0xff, 0xff])
        expect(yield* nextBytes(relay)).toEqual([0x07, 0xff])
        yield* esptool.end
        yield* eventually(
          SubscriptionRef.get(endpoints.doors),
          doors => doors.active === 'pty',
        )
        const back = text('back on the pty\n')
        yield* relay.sendBytes(back)
        expect([...(yield* pty.read(back.length, 3))]).toEqual([...back])
      }),
    ))

  test('a second RFC 2217 client is refused while one holds the door', () =>
    withAgent(relay =>
      Effect.gen(function* () {
        const { endpoints } = yield* attachAndOpen
        yield* rfcClient(endpoints.rfc2217)
        yield* eventually(
          SubscriptionRef.get(endpoints.doors),
          doors => doors.active === 'rfc2217',
        )
        const second = yield* rfcClient(endpoints.rfc2217)
        expect(yield* eventually(second.closed, closed => closed)).toBe(true)
        expect(yield* relay.connections).toBe(1)
      }),
    ))

  test('a recognized reset sends exactly one Reset, resolved by the browser ResetResult', () =>
    withAgent(relay =>
      Effect.gen(function* () {
        const { endpoints } = yield* attachAndOpen
        const notices = yield* Queue.unbounded<string>()
        yield* Effect.forkScoped(
          endpoints.notices.pipe(
            Stream.runForEach(event =>
              event._tag === 'Reset'
                ? Queue.offer(
                    notices,
                    `${event.kind} ${Option.isNone(event.result) ? 'done' : 'failed'}`,
                  )
                : Effect.void,
            ),
          ),
        )
        const esptool = yield* rfcClient(endpoints.rfc2217)
        yield* esptool.replay('D1 R1 P P R1 D1 R0 D1')
        const reset = yield* nextControl(relay)
        expect(reset).toMatchObject({ _tag: 'Reset', kind: 'hardReset' })
        if (reset._tag !== 'Reset') return
        yield* relay.browser({
          _tag: 'ResetResult',
          id: reset.id,
          outcome: 'done',
        })
        expect(
          yield* Queue.take(notices).pipe(Effect.timeout('5 seconds')),
        ).toBe('hardReset done')
        yield* Effect.sleep('200 millis')
        expect(yield* Queue.size(relay.received)).toBe(0)
      }),
    ))

  test('port Lost and Open epochs are reflected in status, and writes while Lost are dropped', () =>
    withAgent(relay =>
      Effect.gen(function* () {
        const { port } = yield* attachAndOpen
        expect(isPortOpen(yield* SubscriptionRef.get(port.status))).toBe(true)
        yield* relay.browser({
          _tag: 'PortChanged',
          status: { epoch: Epoch.make(1), state: { _tag: 'Lost' } },
        })
        const lost = yield* statusUntil(port.status, status =>
          Option.exists(status.port, p => p.state._tag === 'Lost'),
        )
        expect(isPortOpen(lost)).toBe(false)
        expect(yield* port.write(text('into the gap'))).toBe(false)
        yield* relay.browser({
          _tag: 'PortChanged',
          status: {
            epoch: Epoch.make(2),
            state: { _tag: 'Open', baud: 115200 },
          },
        })
        const reopened = yield* statusUntil(port.status, isPortOpen)
        expect(Option.map(reopened.port, p => p.epoch)).toEqual(
          Option.some(Epoch.make(2)),
        )
        expect(reopened.traffic.dropped).toBe(12)
        expect(yield* port.write(text('after'))).toBe(true)
        expect(yield* nextBytes(relay)).toEqual([...text('after')])
      }),
    ))

  test('the control socket verb resets through the browser and reports its refusal', () =>
    withAgent((relay, dir) =>
      Effect.gen(function* () {
        yield* attachAndOpen
        const control = Bridge.sessionPaths(dir, CODE).control
        const answered = Effect.gen(function* () {
          const reset = yield* nextControl(relay)
          if (reset._tag !== 'Reset') return yield* Effect.die(reset)
          yield* relay.browser({
            _tag: 'ResetResult',
            id: reset.id,
            outcome:
              reset.kind === 'usbJtagBootloader' ? 'done' : 'portNotOpen',
          })
        })
        yield* Effect.forkScoped(Effect.forever(answered))
        const reset = (kind: ResetKind) =>
          requestReset({ path: control, code: CODE, kind })
        expect(
          Exit.isSuccess(yield* Effect.exit(reset('usbJtagBootloader'))),
        ).toBe(true)
        const refused = yield* Effect.flip(reset('hardReset'))
        expect(refused).toMatchObject({
          _tag: 'ControlRefused',
          why: { _tag: 'Answered', reply: 'the browser port is not open' },
        })
      }),
    ))

  test('a dropped socket is redialed and welcomed again; replaced stops for good', () =>
    withAgent(relay =>
      Effect.gen(function* () {
        const port = yield* (yield* Remote).attach(CODE)
        yield* relay.closeAgent(1011)
        yield* statusUntil(
          port.status,
          status => status.link._tag === 'Redialing',
        )
        yield* eventually(relay.connections, n => n === 2)
        yield* statusUntil(port.status, status => status.link._tag === 'Joined')
        yield* relay.closeAgent(4410)
        const over = yield* statusUntil(
          port.status,
          status => status.link._tag === 'Over',
        )
        expect(over.link).toEqual({
          _tag: 'Over',
          ending: { _tag: 'Replaced' },
        })
        yield* Effect.sleep('600 millis')
        expect(yield* relay.connections).toBe(2)
      }),
    ))

  for (const [code, tag] of [
    [4401, 'Unauthorized'],
    [4404, 'SessionNotFound'],
    [4426, 'UnsupportedVersion'],
  ] as const) {
    test(`admission close ${code} is ${tag}`, () =>
      withAgent(
        () =>
          Effect.gen(function* () {
            const error = yield* Effect.flip((yield* Remote).attach(CODE))
            expect(error._tag).toBe(tag)
          }),
        { admit: () => code },
      ))
  }

  test('a relay that selects another subprotocol (here the token) is refused before any frame', () =>
    withAgent(
      relay =>
        Effect.gen(function* () {
          const error = yield* Effect.flip((yield* Remote).attach(CODE))
          expect(error._tag === 'RelayUnreachable' && error.why).toMatchObject({
            _tag: 'Dial',
            reason: 'subprotocol',
          })
          expect(yield* relay.connections).toBe(1)
        }),
      { subprotocol: `auth.${TOKEN}` },
    ))

  test('a reset with no agent behind the control socket says so', () =>
    withAgent((_relay, dir) =>
      Effect.gen(function* () {
        const refused = yield* Effect.flip(
          requestReset({
            path: Bridge.sessionPaths(dir, CODE).control,
            code: CODE,
            kind: 'hardReset',
          }),
        )
        expect(refused.why).toEqual({ _tag: 'NoAgent', code: CODE })
      }),
    ))

  test('closing the scope removes the symlink, the control socket and the session dir', async () => {
    const dir = await withAgent((_relay, dir) =>
      attachAndOpen.pipe(Effect.as(dir)),
    )
    expect(await readdir(dir)).toEqual([])
  })
})

const MAIN = `${import.meta.dir}/../src/main.ts`

/** `cycling remote` as the launcher runs it, against the fake relay. */
const cli = (options: {
  readonly relay: FakeRelay
  readonly args: ReadonlyArray<string>
  readonly env?: Readonly<Record<string, string>>
}) =>
  Effect.gen(function* () {
    const dir = yield* Effect.promise(() =>
      mkdtemp(join(tmpdir(), 'agent-cli-')),
    )
    const child = yield* Effect.acquireRelease(
      Effect.sync(() =>
        Bun.spawn([process.execPath, MAIN, ...options.args], {
          env: {
            ...process.env,
            CYCLING_REMOTE_TOKEN: TOKEN,
            CYCLING_REMOTE_DIR: dir,
            CYCLING_REMOTE_RELAY: options.relay.url,
            ...options.env,
          },
          stdout: 'pipe',
          stderr: 'pipe',
        }),
      ),
      child => Effect.sync(() => child.kill()),
    )
    return {
      dir,
      /** Exit code, stdout and stderr, or a failure if it does not exit within `seconds`. */
      exit: (seconds: number) =>
        Effect.promise(() =>
          Promise.all([
            child.exited,
            new Response(child.stdout).text(),
            new Response(child.stderr).text(),
          ]),
        ).pipe(
          Effect.map(([code, stdout, stderr]) => ({ code, stdout, stderr })),
          Effect.timeout(`${seconds} seconds`),
        ),
    }
  })

const runCli = <A, E>(program: Effect.Effect<A, E, Scope.Scope>) =>
  Effect.runPromise(Effect.scoped(program))

describe('main.ts', () => {
  test('the child sees both endpoints and its exit code is mirrored', () =>
    runCli(
      Effect.gen(function* () {
        const relay = yield* fakeRelay({ snapshot: OPEN_SNAPSHOT })
        const agent = yield* cli({
          relay,
          args: [
            '7kqm-4xp2',
            '--',
            'sh',
            '-c',
            'test -L "$CYCLING_PORT" || exit 1; case "$CYCLING_ESPTOOL_PORT" in rfc2217://127.0.0.1:*) exit 7;; esac; exit 1',
          ],
        })
        const { code, stderr } = yield* agent.exit(10)
        expect(stderr).toContain('device   usb-serial 303a:1001')
        expect(code).toBe(7)
        expect(yield* Effect.promise(() => readdir(agent.dir))).toEqual([])
      }),
    ))

  test('L3: the child never sees the relay token, under any name', () =>
    runCli(
      Effect.gen(function* () {
        const relay = yield* fakeRelay({ snapshot: OPEN_SNAPSHOT })
        const agent = yield* cli({
          relay,
          args: [CODE, '--', 'env'],
          env: { AGENT_TOKEN: TOKEN, KEPT: 'yes' },
        })
        const { code, stdout } = yield* agent.exit(10)
        expect(code).toBe(0)
        expect(stdout).toContain('CYCLING_PORT=')
        expect(stdout).toContain('KEPT=yes')
        expect(stdout).not.toContain(TOKEN)
        expect(stdout).not.toContain('CYCLING_REMOTE_TOKEN')
      }),
    ))

  for (const [phase, snapshot] of [
    ['for a device', { ...OPEN_SNAPSHOT, device: null, port: null }],
    ['for the port to open', { ...OPEN_SNAPSHOT, port: null }],
  ] as const) {
    for (const [close, exit, said] of [
      [4408, 0, 'session ended: the session expired'],
      [4401, 2, 'the relay refused this agent token'],
    ] as const) {
      test(`M2: a session that ends (${close}) while waiting ${phase} exits ${exit} at once`, () =>
        runCli(
          Effect.gen(function* () {
            const relay = yield* fakeRelay({ snapshot })
            const agent = yield* cli({
              relay,
              args: [CODE, '--', 'sh', '-c', 'exit 9'],
            })
            yield* eventually(relay.connections, n => n === 1)
            yield* Effect.sleep('300 millis')
            yield* relay.closeAgent(close)
            const { code, stderr } = yield* agent.exit(3)
            expect(stderr).toContain(said)
            expect(code).toBe(exit)
          }),
        ))
    }
  }

  test('a describe-only USB session prints the descriptor as JSON and exits 0', () =>
    runCli(
      Effect.gen(function* () {
        const descriptor = {
          vendorId: 0x22d9,
          productId: 0x2765,
          usbVersion: '2.0',
          deviceClass: 6,
          deviceSubclass: 1,
          deviceProtocol: 1,
          deviceVersion: '4.4.0',
          productName: 'Camera',
        }
        const relay = yield* fakeRelay({
          snapshot: {
            ...OPEN_SNAPSHOT,
            device: { _tag: 'usb', descriptor },
            port: null,
          },
        })
        const agent = yield* cli({ relay, args: [CODE] })
        const { code, stdout } = yield* agent.exit(10)
        expect(code).toBe(0)
        expect(JSON.parse(stdout)).toEqual(descriptor)
      }),
    ))
})
