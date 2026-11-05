import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Layer from 'effect/Layer'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Random from 'effect/Random'
import * as Schema from 'effect/Schema'
import * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import * as Socket from 'effect/socket/Socket'

/**
 * Black-box check of a running relay: `bun web/relay/test/smoke.ts <relay-url>`.
 * The URL may be http(s) or ws(s). `AGENT_TOKEN` is the secret the relay was
 * started with (at least 32 characters) and `ORIGIN` the browser origin it
 * allows; both default to the local dev values. Exits 0 when every step holds.
 *
 * With `--bad-token` it checks instead a relay started with an unusable
 * `AGENT_TOKEN` (the one in the environment): no agent gets in.
 */
import {
  agentProtocols,
  type BrowserMsg,
  browserProtocols,
  CloseCode,
  closeName,
  type Code,
  KEEPALIVE,
  MAX_FRAME_BYTES,
  mintCode,
  mintOwnerKey,
  type OwnerKey,
  type Receives,
  RequestId,
  type RelayEvent,
  relaySocket,
  type Role,
  type Sends,
  sessionPath,
  SUBPROTOCOL,
  type ToAgent,
  type ToBrowser,
} from '@cycling/protocol'

const base = (process.argv[2] ?? '').replace(/^http/, 'ws').replace(/\/$/, '')
const AGENT_TOKEN =
  process.env['AGENT_TOKEN'] ?? 'smoke-agent-token-of-32-characters'
const ORIGIN = process.env['ORIGIN'] ?? 'http://localhost:5173'
const WAIT = '3 seconds'

class SmokeFailure extends Schema.TaggedError<
  SmokeFailure,
  { readonly brand: unique symbol }
>()('SmokeFailure', { message: Schema.String }) {}

const fail = (message: string) => Effect.fail(new SmokeFailure({ message }))
const say = (line: string) =>
  Effect.sync(() => process.stdout.write(`${line}\n`))

interface Dial<R extends Role> {
  readonly role: R
  readonly path: string
  readonly protocols: ReadonlyArray<string>
  readonly ip: string
  readonly origin?: string | undefined
}

/**
 * The global WebSocket constructor, plus the handshake headers a browser
 * would send for free: the relay keys its rate limit on `cf-connecting-ip`
 * and checks `Origin`. `Socket.layerWebSocketConstructorGlobal` rejects
 * headers, so each dial gets its own constructor.
 */
const dialer = (dial: Dial<Role>) =>
  Layer.succeed(Socket.WebSocketConstructor)((url, options) => {
    const headers: Record<string, string> = { 'cf-connecting-ip': dial.ip }
    if (dial.origin !== undefined) {
      headers['origin'] = dial.origin
    }
    const client =
      options === undefined ||
      typeof options === 'string' ||
      Array.isArray(options)
        ? { protocols: options }
        : options
    return new globalThis.WebSocket(url, { ...client, headers })
  })

const dialFailure = (label: string, detail: string) =>
  new SmokeFailure({ message: `${label}: ${detail}` })

type Heard<R extends Role> = RelayEvent<Receives<R>>

interface Listener {
  readonly label: string
  readonly next: Effect.Effect<RelayEvent<ToBrowser | ToAgent>, SmokeFailure>
}

interface Peer<R extends Role> extends Listener {
  readonly send: (item: Uint8Array | Sends<R>) => Effect.Effect<boolean>
  readonly next: Effect.Effect<Heard<R>, SmokeFailure>
  readonly leave: Effect.Effect<void>
}

const within = <A>(label: string, inbox: Queue.Queue<A>) =>
  Queue.take(inbox).pipe(
    Effect.timeoutOrElse({
      duration: WAIT,
      orElse: () => fail(`${label}: nothing arrived within ${WAIT}`),
    }),
  )

/** A relay client for one role, through the protocol's own `relaySocket`. */
const connect = <R extends Role>(label: string, dial: Dial<R>) =>
  Effect.gen(function* () {
    const scope = yield* Scope.fork(yield* Effect.scope)
    const socket = yield* relaySocket({
      role: dial.role,
      url: `${base}${dial.path}`,
      protocols: dial.protocols,
    }).pipe(
      Effect.provide(dialer(dial)),
      Scope.provide(scope),
      Effect.mapError(error =>
        dialFailure(label, `${error.reason}, ${error.detail}`),
      ),
    )
    const inbox = yield* Queue.unbounded<Heard<R>>()
    yield* Stream.runForEach(socket.events, event =>
      Queue.offer(inbox, event),
    ).pipe(Effect.forkIn(scope))
    const peer: Peer<R> = {
      label,
      send: socket.send,
      next: within(label, inbox),
      leave: Scope.close(scope, Exit.void),
    }
    return peer
  })

type RawChunk = string | Uint8Array | { readonly closeCode: number }

interface RawPeer {
  readonly send: (frame: string | Uint8Array) => Effect.Effect<void>
  readonly next: Effect.Effect<RawChunk, SmokeFailure>
}

const closeCodeOf = (error: Socket.SocketError): number =>
  error.reason._tag === 'SocketCloseError' ? error.reason.code : 1006

/**
 * A bare `effect/socket` client for the frames `relaySocket` will not send
 * (wrong direction, malformed, `ping`) and the handshakes it will not accept
 * (no version token selected).
 */
const raw = (label: string, dial: Dial<Role>) =>
  Effect.gen(function* () {
    const socket = yield* Socket.makeWebSocket(`${base}${dial.path}`, {
      protocols: [...dial.protocols],
    }).pipe(Effect.provide(dialer(dial)))
    const { pull } = yield* socket.reader.pipe(
      Effect.mapError(error => dialFailure(label, error.message)),
    )
    const writer = yield* socket.writer
    const inbox = yield* Queue.unbounded<RawChunk>()
    yield* Stream.fromEffectRepeat(pull).pipe(
      Stream.flattenIterable,
      Stream.runForEach(chunk => Queue.offer(inbox, chunk)),
      Effect.catchTag('SocketError', error =>
        Queue.offer(inbox, { closeCode: closeCodeOf(error) }),
      ),
      Effect.forkScoped,
    )
    const peer: RawPeer = {
      send: frame => writer.write(frame).pipe(Effect.orDie),
      next: within(label, inbox),
    }
    return peer
  })

const rawClose = (peer: RawPeer) =>
  peer.next.pipe(
    Effect.repeat({
      until: chunk => typeof chunk === 'object' && 'closeCode' in chunk,
    }),
    Effect.map(chunk =>
      typeof chunk === 'object' && 'closeCode' in chunk ? chunk.closeCode : -1,
    ),
  )

const subset = (actual: unknown, expected: unknown): boolean => {
  if (
    typeof expected !== 'object' ||
    expected === null ||
    typeof actual !== 'object' ||
    actual === null
  ) {
    return actual === expected
  }
  return Object.entries(expected).every(([key, value]) =>
    subset(Reflect.get(actual, key), value),
  )
}

const control = (peer: Listener): Effect.Effect<unknown, SmokeFailure> =>
  peer.next.pipe(
    Effect.flatMap(event =>
      event._tag === 'Control'
        ? Effect.succeed(event.message)
        : fail(`${peer.label}: expected control, got ${JSON.stringify(event)}`),
    ),
  )

const expectMessage = (peer: Listener, label: string, expected: unknown) =>
  control(peer).pipe(
    Effect.flatMap(message =>
      subset(message, expected)
        ? say(`  ok   ${label}`)
        : fail(`${label}: got ${JSON.stringify(message)}`),
    ),
  )

const nameOf = (code: number) =>
  Option.getOrElse(closeName(code), () => 'unnamed')

const expectClosed = (peer: Listener, label: string, code: number) =>
  peer.next.pipe(
    Effect.flatMap(event =>
      event._tag === 'Closed' && event.code === code
        ? say(`  ok   ${label} (close ${code} ${nameOf(code)})`)
        : fail(`${label}: wanted close ${code}, got ${JSON.stringify(event)}`),
    ),
  )

const check = (label: string, ok: boolean, detail?: unknown) =>
  ok
    ? say(`  ok   ${label}`)
    : fail(
        `${label}${detail === undefined ? '' : ` ${JSON.stringify(detail)}`}`,
      )

const expectBytes = (peer: Listener, label: string, sent: Uint8Array) =>
  peer.next.pipe(
    Effect.flatMap(event =>
      event._tag === 'Bytes' &&
      event.bytes.length === sent.length &&
      event.bytes.every((byte, index) => byte === sent[index])
        ? say(`  ok   ${label} (${sent.length} bytes, byte-exact)`)
        : fail(`${label}: bytes differ, got ${JSON.stringify(event)}`),
    ),
  )

const device: BrowserMsg & { readonly _tag: 'DeviceAnnounced' } = {
  _tag: 'DeviceAnnounced',
  device: {
    _tag: 'serial',
    identity: { vendorId: 0x303a, productId: 0x1001 },
  },
}
const requestId = Schema.decodeSync(RequestId)(7)

interface Session {
  readonly code: Code
  readonly key: OwnerKey
  readonly ip: string
}

const session = Effect.gen(function* () {
  const bytes = yield* Effect.all(
    Array.from({ length: 23 }, () => Random.nextIntBetween(0, 256)),
  )
  return {
    code: mintCode(Uint8Array.from(bytes.slice(0, 5))),
    key: mintOwnerKey(Uint8Array.from(bytes.slice(5, 21))),
    ip: `10.${bytes[21]}.${bytes[22]}.${bytes[0]}`,
  }
})

const browserDial = (s: Session, key = s.key): Dial<'browser'> => ({
  role: 'browser',
  path: sessionPath('browser', s.code),
  protocols: browserProtocols(key),
  ip: s.ip,
  origin: ORIGIN,
})
const agentDial = (s: Session, token = AGENT_TOKEN): Dial<'agent'> => ({
  role: 'agent',
  path: sessionPath('agent', s.code),
  protocols: agentProtocols(token),
  ip: s.ip,
})
const browser = (label: string, s: Session, key = s.key) =>
  connect(label, browserDial(s, key))
const agent = (label: string, s: Session, token = AGENT_TOKEN) =>
  connect(label, agentDial(s, token))

const get = (path: string) =>
  Effect.tryPromise({
    try: () => fetch(`${base.replace(/^ws/, 'http')}${path}`),
    catch: () => new SmokeFailure({ message: `GET ${path} failed` }),
  })

const happyPath = Effect.gen(function* () {
  yield* say('pairing, presence, persistence, bytes')
  const s = yield* session
  const a = yield* browser('browser A', s)
  yield* expectMessage(a, 'browser A gets Welcome with an empty snapshot', {
    _tag: 'Welcome',
    role: 'browser',
    snapshot: {
      device: null,
      port: null,
      browserPresent: true,
      agentPresent: false,
    },
  })
  yield* a.send(device)

  const g = yield* agent('agent A', s)
  yield* expectMessage(g, 'late agent A sees the persisted device in Welcome', {
    _tag: 'Welcome',
    role: 'agent',
    snapshot: {
      device: device.device,
      browserPresent: true,
      agentPresent: true,
    },
  })
  yield* expectMessage(a, 'browser A hears PeerPresence agent=true', {
    _tag: 'PeerPresence',
    role: 'agent',
    present: true,
  })

  yield* a.send(device)
  yield* expectMessage(g, 'agent A receives the live DeviceAnnounced', device)

  const bytes = Uint8Array.from({ length: 256 }, (_, index) => index)
  yield* a.send(bytes)
  yield* expectBytes(g, 'browser to agent', bytes)
  yield* g.send(bytes)
  yield* expectBytes(a, 'agent to browser', bytes)

  const b = yield* browser('browser B (same key)', s)
  yield* expectMessage(b, 'browser B resumes with the agent present', {
    _tag: 'Welcome',
    role: 'browser',
    snapshot: { device: device.device, agentPresent: true },
  })
  yield* expectClosed(a, 'browser A is replaced', CloseCode.replaced)
  yield* expectMessage(g, 'agent A hears PeerPresence browser=true', {
    _tag: 'PeerPresence',
    role: 'browser',
    present: true,
  })
  yield* b.send({ _tag: 'ResetResult', id: requestId, outcome: 'done' })
  yield* expectMessage(
    g,
    'the replaced socket closing did not announce the browser gone',
    { _tag: 'ResetResult', outcome: 'done' },
  )

  const h = yield* agent('agent B (replaces A)', s)
  yield* expectClosed(g, 'agent A is replaced', CloseCode.replaced)
  yield* expectMessage(h, 'agent B gets Welcome', {
    _tag: 'Welcome',
    role: 'agent',
  })
  yield* expectMessage(b, 'browser B hears PeerPresence agent=true again', {
    _tag: 'PeerPresence',
    role: 'agent',
    present: true,
  })
  yield* h.send({ _tag: 'Reset', id: requestId, kind: 'hardReset' })
  yield* expectMessage(b, 'agent request reaches the browser', {
    _tag: 'Reset',
    kind: 'hardReset',
  })

  yield* h.leave
  yield* expectMessage(b, 'browser B hears PeerPresence agent=false', {
    _tag: 'PeerPresence',
    role: 'agent',
    present: false,
  })

  const i = yield* agent('agent C', s)
  yield* expectMessage(i, 'agent C gets Welcome', {
    _tag: 'Welcome',
    role: 'agent',
  })
  yield* expectMessage(b, 'browser B hears agent C', {
    _tag: 'PeerPresence',
    role: 'agent',
    present: true,
  })
  yield* b.send({ _tag: 'Bye' })
  yield* expectClosed(
    b,
    'browser Bye closes the browser with visitorDisconnected',
    CloseCode.visitorDisconnected,
  )
  yield* expectClosed(
    i,
    'agent C is closed with visitorDisconnected, no Ended message first',
    CloseCode.visitorDisconnected,
  )

  const late = yield* agent('agent after the end', s)
  yield* expectClosed(
    late,
    'an ended code is unknown to agents',
    CloseCode.notFound,
  )
})

const keepalive = Effect.gen(function* () {
  yield* say('keepalive')
  const s = yield* session
  const peer = yield* raw('browser', browserDial(s))
  yield* peer.next
  yield* peer.send(KEEPALIVE.ping)
  const pong = yield* peer.next
  yield* check(
    'ping is answered with pong on the same socket',
    pong === KEEPALIVE.pong,
    pong,
  )
})

const violations = Effect.gen(function* () {
  yield* say('protocol violations close only the sender')
  const cases: ReadonlyArray<{
    readonly label: string
    readonly sender: 'browser' | 'agent'
    readonly payload: string | Uint8Array
  }> = [
    {
      label: 'browser sends an agent message',
      sender: 'browser',
      payload: JSON.stringify({ _tag: 'Reset', id: 1, kind: 'hardReset' }),
    },
    {
      label: 'agent sends a browser message',
      sender: 'agent',
      payload: JSON.stringify({ _tag: 'Bye' }),
    },
    {
      label: 'browser announces a device with a serialNumber',
      sender: 'browser',
      payload: JSON.stringify({
        _tag: 'DeviceAnnounced',
        device: {
          _tag: 'serial',
          identity: { vendorId: 1, productId: 2, serialNumber: 'AA:BB' },
        },
      }),
    },
    {
      label: 'browser sends text that is not JSON',
      sender: 'browser',
      payload: 'hello',
    },
    {
      label: 'agent sends a binary frame over the limit',
      sender: 'agent',
      payload: new Uint8Array(MAX_FRAME_BYTES + 1),
    },
  ]
  for (const { label, sender, payload } of cases) {
    const s = yield* session
    const [offender, bystander] =
      sender === 'browser'
        ? yield* Effect.gen(function* () {
            const offender = yield* raw('offending browser', browserDial(s))
            yield* offender.next
            const bystander = yield* agent('agent', s)
            yield* bystander.next
            return [offender, bystander] as const
          })
        : yield* Effect.gen(function* () {
            const bystander = yield* browser('browser', s)
            yield* bystander.next
            const offender = yield* raw('offending agent', agentDial(s))
            yield* offender.next
            yield* bystander.next
            return [offender, bystander] as const
          })
    yield* offender.send(payload)
    const code = yield* rawClose(offender)
    yield* check(
      `${label}: closed with protocolViolation`,
      code === CloseCode.protocolViolation,
      code,
    )
    yield* expectMessage(
      bystander,
      `${label}: the other side only sees the sender leave`,
      { _tag: 'PeerPresence', role: sender, present: false },
    )
  }
})

const admission = Effect.gen(function* () {
  yield* say('admission')
  const s = yield* session
  const a = yield* browser('browser', s)
  yield* a.next

  const badToken = yield* agent('agent with a bad token', s, 'wrong')
  yield* expectClosed(badToken, 'a bad token', CloseCode.unauthorized)

  const intruder = yield* browser(
    'browser with another key',
    s,
    mintOwnerKey(new Uint8Array(16).fill(1)),
  )
  yield* expectClosed(intruder, 'a second owner key', CloseCode.codeInUse)

  const unknown = yield* agent('agent on an unclaimed code', yield* session)
  yield* expectClosed(
    unknown,
    'an agent cannot create a session',
    CloseCode.notFound,
  )

  const s2 = yield* session
  const noVersion = yield* raw('browser without a version', {
    ...browserDial(s2),
    protocols: [],
  })
  const versionCode = yield* rawClose(noVersion)
  yield* check(
    'a missing version token is refused with unsupportedVersion',
    versionCode === CloseCode.unsupportedVersion,
    versionCode,
  )

  const foreign = yield* connect('browser from a foreign origin', {
    ...browserDial(s2),
    origin: 'https://evil.example',
  })
  yield* expectClosed(foreign, 'a foreign origin', CloseCode.unauthorized)

  const noKey = yield* connect('browser without an owner key', {
    ...browserDial(s2),
    protocols: [SUBPROTOCOL],
  })
  yield* expectClosed(noKey, 'a missing owner key', CloseCode.unauthorized)

  const badCode = yield* connect('agent with a malformed code', {
    ...agentDial(s2),
    path: sessionPath('agent', s2.code).replace(s2.code, 'nope'),
  })
  yield* expectClosed(badCode, 'a malformed code', CloseCode.notFound)

  const plain = yield* get(sessionPath('agent', s.code))
  yield* check(
    'a non-upgrade request on a session path gets 426',
    plain.status === 426,
    plain.status,
  )
  const missing = yield* get('/elsewhere')
  yield* check(
    'any other path gets 404',
    missing.status === 404,
    missing.status,
  )
})

const rateLimit = Effect.gen(function* () {
  yield* say('rate limit')
  const ip = '10.9.9.9'
  const results: Array<number> = []
  for (let index = 0; index < 30; index++) {
    const s = yield* session
    const peer = yield* connect('browser', { ...browserDial(s), ip })
    const first = yield* peer.next
    results.push(first._tag === 'Closed' ? first.code : 0)
  }
  const limited = results.filter(code => code === CloseCode.rateLimited).length
  yield* check(
    'browser opens beyond the per-address ceiling are refused with rateLimited',
    limited > 0 && results.slice(0, 10).every(code => code === 0),
    results,
  )
})

const badToken = Effect.gen(function* () {
  yield* say('a relay whose AGENT_TOKEN is unusable')
  const s = yield* session
  const a = yield* browser('browser', s)
  yield* expectMessage(a, 'browsers are still admitted', {
    _tag: 'Welcome',
    role: 'browser',
  })
  for (const [label, token] of [
    ['the configured token', AGENT_TOKEN],
    ['an empty token', ''],
  ] as const) {
    const peer = yield* agent(`agent with ${label}`, s, token)
    yield* expectClosed(peer, `${label} is refused`, CloseCode.unauthorized)
  }
})

const program = Effect.gen(function* () {
  if (base === '') {
    return yield* fail(
      'usage: bun web/relay/test/smoke.ts <relay-url> [--bad-token]',
    )
  }
  if (process.argv[3] === '--bad-token') {
    return yield* Effect.scoped(badToken)
  }
  yield* Effect.scoped(happyPath)
  yield* Effect.scoped(keepalive)
  yield* Effect.scoped(violations)
  yield* Effect.scoped(admission)
  yield* Effect.scoped(rateLimit)
  yield* say('all steps held')
})

void Effect.runPromiseExit(program).then(exit => {
  if (exit._tag === 'Failure') {
    process.stderr.write(`FAILED: ${String(exit.cause)}\n`)
    process.exit(1)
  }
  process.exit(0)
})
