import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import * as Queue from 'effect/Queue'
import * as Random from 'effect/Random'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import * as Stream from 'effect/Stream'
import * as Socket from 'effect/socket/Socket'

import {
  browserProtocols,
  type Code,
  Epoch,
  mintCode,
  mintOwnerKey,
  relaySocket,
  sessionPath,
} from '@cycling/protocol'

/**
 * The whole chain against a running relay, with no device and no Chrome:
 * a fake browser (scripted C606 behind it) holds a session, and the real
 * `cycling remote` launcher runs a devtools-shaped tty client through it.
 *
 *   bun web/agent/test/e2e-relay.ts <relay-url>
 *
 * Needs `AGENT_TOKEN` (the relay's secret, at least 32 characters), `ORIGIN`
 * (an allowed origin), and bun, python3 and uv on PATH. Exits 0 when every
 * step holds.
 */

const base = (process.argv[2] ?? '').replace(/^http/, 'ws').replace(/\/$/, '')
const AGENT_TOKEN = process.env['AGENT_TOKEN'] ?? ''
const ORIGIN = process.env['ORIGIN'] ?? 'http://localhost:5173'
const ROOT = new URL('../../..', import.meta.url).pathname

class E2eFailure extends Schema.TaggedError<
  E2eFailure,
  { readonly brand: unique symbol }
>()('E2eFailure', { message: Schema.String }) {}

const fail = (message: string) => Effect.fail(new E2eFailure({ message }))
const say = (line: string) =>
  Effect.sync(() => process.stdout.write(`${line}\n`))
const check = (label: string, ok: boolean, detail: unknown) =>
  ok ? say(`  ok   ${label}`) : fail(`${label}: ${JSON.stringify(detail)}`)

/** A browser sends the page's Origin, which the relay checks; Bun's WebSocket can set it. */
const withOrigin = Layer.succeed(Socket.WebSocketConstructor)(
  (url, options) =>
    new WebSocket(url, {
      protocols:
        typeof options === 'string'
          ? [options]
          : Array.isArray(options)
            ? options
            : [],
      headers: { origin: ORIGIN },
    }),
)

const encoder = new TextEncoder()
const reply = (id: string) =>
  `${JSON.stringify({ type: 'reply', id: Number(id), status: 'OK', data: 'device=fake-c606' })}\n`

/** Answers `CMD <id> INFO` with one reply line and echoes any other line. */
const fakeBrowser = Effect.gen(function* () {
  const random = yield* Effect.forEach(Array.from({ length: 21 }), () =>
    Random.nextIntBetween(0, 256),
  )
  const bytes = Uint8Array.from(random)
  const code = mintCode(bytes.subarray(0, 5))
  const ownerKey = mintOwnerKey(bytes.subarray(5))
  const socket = yield* relaySocket({
    role: 'browser',
    url: `${base}${sessionPath('browser', code)}`,
    protocols: browserProtocols(ownerKey),
  }).pipe(
    Effect.provide(withOrigin),
    Effect.mapError(
      error =>
        new E2eFailure({
          message: `the fake browser could not reach the relay: ${error.detail}`,
        }),
    ),
  )
  const fromAgent = yield* Ref.make(0)
  const toDevice = yield* Queue.unbounded<Uint8Array>()
  yield* Effect.forkScoped(
    socket.events.pipe(
      Stream.runForEach(event => {
        if (event._tag === 'Console') {
          return Ref.update(fromAgent, n => n + event.bytes.byteLength).pipe(
            Effect.andThen(Queue.offer(toDevice, event.bytes)),
          )
        }
        return Effect.void
      }),
    ),
  )
  yield* Effect.forkScoped(
    Stream.fromQueue(toDevice).pipe(
      Stream.decodeText,
      Stream.splitLines,
      Stream.runForEach(line => {
        const info = /^CMD (\d+) INFO$/.exec(line.trim())
        return socket.send({
          _tag: 'Console',
          bytes: encoder.encode(
            info ? reply(info[1] ?? '0') : `${line.trim()}\n`,
          ),
        })
      }),
    ),
  )
  yield* socket.send({
    _tag: 'DeviceAnnounced',
    device: {
      _tag: 'serial',
      identity: { vendorId: 0x303a, productId: 0x1001 },
    },
  })
  yield* socket.send({
    _tag: 'PortChanged',
    status: { epoch: Epoch.make(1), state: { _tag: 'Open', baud: 115200 } },
  })
  return {
    code,
    fromAgent: Ref.get(fromAgent),
  }
})

/** `cycling remote <code> -- <cmd>`, through the same launcher the dev shell's `cycling` runs. */
const cyclingRemote = (code: Code, command: ReadonlyArray<string>) =>
  Effect.tryPromise({
    try: async () => {
      const child = Bun.spawn(
        [
          'uv',
          'run',
          '--project',
          'tools/devtools',
          '--locked',
          'python',
          '-m',
          'cycling_devtools.cli',
          'remote',
          code,
          '--',
          ...command,
        ],
        {
          cwd: ROOT,
          env: {
            ...process.env,
            CYCLING_REMOTE_TOKEN: AGENT_TOKEN,
            CYCLING_REMOTE_RELAY: base,
          },
          stdout: 'pipe',
          stderr: 'pipe',
        },
      )
      const [stdout, stderr, exitCode] = await Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited,
      ])
      return { stdout, stderr, exitCode }
    },
    catch: error => new E2eFailure({ message: String(error) }),
  })

const TTY_CLIENT = `
import os, select, sys, termios, tty, time
fd = os.open(os.environ['CYCLING_PORT'], os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
attrs = termios.tcgetattr(fd)
tty.cfmakeraw(attrs)
attrs[2] = (attrs[2] | termios.CLOCAL | termios.CREAD) & ~termios.HUPCL
attrs[4] = attrs[5] = termios.B115200
termios.tcsetattr(fd, termios.TCSANOW, attrs)
os.write(fd, b'CMD 41 INFO\\n')
data, deadline = b'', time.monotonic() + 8
while b'\\n' not in data and time.monotonic() < deadline:
    if select.select([fd], [], [], 0.2)[0]:
        data += os.read(fd, 4096)
os.close(fd)
sys.stdout.write(data.decode())
sys.exit(0 if b'"id": 41' in data or b'"id":41' in data else 1)
`

const ttyThroughPty = Effect.scoped(
  Effect.gen(function* () {
    const browser = yield* fakeBrowser
    yield* say(`pty: session ${browser.code}`)
    const run = yield* cyclingRemote(browser.code, [
      'python3',
      '-c',
      TTY_CLIENT,
    ])
    yield* say(`  reply: ${run.stdout.trim()}`)
    yield* check(
      'an open_no_reset client on $CYCLING_PORT gets the device reply to CMD 41 INFO',
      run.exitCode === 0,
      run,
    )
  }),
)

const program = Effect.gen(function* () {
  if (base === '') {
    return yield* fail('usage: bun web/agent/test/e2e-relay.ts <relay-url>')
  }
  if (AGENT_TOKEN.length < 32) {
    return yield* fail(
      'AGENT_TOKEN must be the relay secret, at least 32 characters',
    )
  }
  yield* ttyThroughPty
  yield* say('all steps held')
})

void Effect.runPromiseExit(program).then(exit => {
  if (exit._tag === 'Failure') {
    process.stderr.write(`FAILED: ${String(exit.cause)}\n`)
    process.exit(1)
  }
  process.exit(0)
})
