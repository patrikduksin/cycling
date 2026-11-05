import * as Cause from 'effect/Cause'
import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import { type Session, md5Hex } from '@cycling/flasher'
import {
  type BrowserMsg,
  CloseCode,
  Lease,
  Md5,
  type Op,
  type RelayEvent,
  RequestId,
  SerialIdentity,
  type ToBrowser,
  UsbDescriptor,
  opDataFrames,
} from '@cycling/protocol'

import {
  type Adapters,
  ChooserFailed,
  type RelayLink,
  ReacquireTimeout,
  SerialLinkLost,
  type SerialLink,
} from '../links.ts'
import { demoFlash } from './image.ts'
import { DEMO_CHIP, memorySession } from './memorySession.ts'

/**
 * A device and a relay that exist only in the dev server (`?fake`), so the
 * Live screen and the byte monitor can be seen without hardware or a Worker.
 * `?fake=reset` also has the device reset itself after about five seconds,
 * which walks the port through Lost and Reacquiring.
 * `?fake=fail` does the same, but the device never comes back, so the page
 * shows the failed port and "Choose device again". Disconnect closes the
 * fake socket the way the relay does, with `visitorDisconnected`.
 * `?fake=refuse` has the chooser fail, so Idle shows its notice.
 * `?fake=flash` has a pretend developer put the device into update mode and
 * run Enter, two Reads, a Write (which asks the visitor), a Digest and a
 * Reboot against an in-memory flash, so every update-mode panel can be seen.
 */

const text = new TextEncoder()

const identity = Schema.decodeSync(SerialIdentity)({
  vendorId: 0x303a,
  productId: 0x1001,
})

const descriptor = Schema.decodeSync(UsbDescriptor)({
  vendorId: 0x22d9,
  productId: 0x2765,
  usbVersion: '2.0.0',
  deviceClass: 6,
  deviceSubclass: 1,
  deviceProtocol: 1,
  deviceVersion: '4.1.0',
  manufacturerName: 'Fake Phone Co',
  productName: 'Fake Phone',
})

const fakeSerialLink = (
  resets: boolean,
): Effect.Effect<SerialLink, never, Scope.Scope> =>
  Effect.gen(function* () {
    const output = yield* Queue.unbounded<
      Uint8Array,
      SerialLinkLost | Cause.Done
    >()
    yield* Effect.forkScoped(
      Effect.forEach(
        Array.from({ length: 10_000 }, (_, count) => count),
        count =>
          Effect.sleep(Duration.millis(450)).pipe(
            Effect.andThen(
              resets && count === 11
                ? Queue.fail(output, new SerialLinkLost())
                : Queue.offer(
                    output,
                    text.encode(`{"uptime_ms":${count * 450},"battery":87}\n`),
                  ),
            ),
          ),
        { discard: true },
      ),
    )
    return {
      input: Stream.fromQueue(output),
      write: bytes => Queue.offer(output, bytes).pipe(Effect.asVoid),
    }
  })

const welcome = (agentPresent: boolean): ToBrowser => ({
  _tag: 'Welcome',
  role: 'browser',
  snapshot: {
    device: null,
    port: null,
    expiresAt: 4_000_000_000_000,
    browserPresent: true,
    agentPresent,
  },
})

const control = (message: ToBrowser): RelayEvent<ToBrowser> => ({
  _tag: 'Control',
  message,
})

/** Stands in for the closed SerialPort `granted` hands the flasher; the fake flasher never opens it. */
class GrantedPort extends EventTarget implements SerialPort {
  onconnect = null
  ondisconnect = null
  readonly connected = true
  readonly readable = null
  readonly writable = null
  open() {
    return Promise.resolve()
  }
  setSignals() {
    return Promise.resolve()
  }
  getSignals() {
    return Promise.resolve({
      dataCarrierDetect: false,
      clearToSend: false,
      ringIndicator: false,
      dataSetReady: false,
    })
  }
  getInfo() {
    return { usbVendorId: identity.vendorId, usbProductId: identity.productId }
  }
  close() {
    return Promise.resolve()
  }
  forget() {
    return Promise.resolve()
  }
}

const lease = Lease.make('fakeDeveloper001')

const PAYLOAD = { address: 0x760000, size: 0x1e0000 }

const payload = (): Uint8Array =>
  new Uint8Array(PAYLOAD.size).map((_, index) => (index * 7) & 0xff)

/** The ops `cycling flash` would send, roughly: enter, look, write the other slot, check it, restart. */
const developerScript = (bytes: Uint8Array): ReadonlyArray<Op> => [
  { _tag: 'Enter', lease },
  { _tag: 'Read', lease, region: { address: 0x8000, size: 0x8000 } },
  { _tag: 'Read', lease, region: { address: 0x10000, size: 0x200000 } },
  { _tag: 'Write', lease, region: PAYLOAD, md5: Md5.make(md5Hex(bytes)) },
  { _tag: 'Digest', lease, region: PAYLOAD },
  { _tag: 'Reboot', lease },
]

/** Plays the developer: one op at a time, each sent once the last is answered. */
const playDeveloper = (
  inbound: Queue.Queue<RelayEvent<ToBrowser>, Cause.Done>,
  results: Queue.Queue<BrowserMsg>,
) => {
  const bytes = payload()
  const answered = (id: RequestId): Effect.Effect<void> =>
    Queue.take(results).pipe(
      Effect.flatMap(message =>
        message._tag === 'OpResult' && message.id === id
          ? Effect.void
          : answered(id),
      ),
    )
  return Effect.forEach(
    developerScript(bytes),
    (op, index) => {
      const id = RequestId.make(index + 1)
      return Queue.offer(inbound, control({ _tag: 'Op', id, op })).pipe(
        Effect.andThen(
          op._tag === 'Write'
            ? Effect.forEach(
                opDataFrames(id, bytes),
                frame => Queue.offer(inbound, frame),
                { discard: true },
              )
            : Effect.void,
        ),
        Effect.andThen(answered(id)),
        Effect.andThen(Effect.sleep(Duration.millis(1500))),
      )
    },
    { discard: true },
  )
}

const fakeRelayLink = (
  developer: boolean,
): Effect.Effect<RelayLink, never, Scope.Scope> =>
  Effect.gen(function* () {
    const inbound = yield* Queue.unbounded<RelayEvent<ToBrowser>, Cause.Done>()
    const results = yield* Queue.unbounded<BrowserMsg>()
    yield* Effect.forkScoped(
      Effect.gen(function* () {
        yield* Queue.offer(inbound, control(welcome(false)))
        yield* Effect.sleep(Duration.millis(1500))
        yield* Queue.offer(
          inbound,
          control({ _tag: 'PeerPresence', role: 'agent', present: true }),
        )
        if (developer) {
          yield* Effect.sleep(Duration.millis(1500))
          return yield* playDeveloper(inbound, results)
        }
        yield* Effect.forEach(
          Array.from({ length: 10_000 }, (_, count) => count),
          count =>
            Effect.sleep(Duration.millis(1200)).pipe(
              Effect.andThen(
                Queue.offer(inbound, {
                  _tag: 'Console',
                  bytes: text.encode(`INFO ${count}\n`),
                }),
              ),
            ),
          { discard: true },
        )
      }),
    )
    return {
      events: Stream.fromQueue(inbound),
      send: item =>
        item._tag === 'Bye'
          ? Queue.offer(inbound, {
              _tag: 'Closed',
              code: CloseCode.visitorDisconnected,
            }).pipe(Effect.andThen(Queue.end(inbound)))
          : item._tag === 'OpResult'
            ? Queue.offer(results, item)
            : Effect.succeed(true),
    }
  })

const fakeFlasher = (flash: Uint8Array) => ({
  open: (): Effect.Effect<Session> =>
    Effect.sleep(Duration.millis(1500)).pipe(
      Effect.andThen(
        memorySession({
          flash,
          chip: DEMO_CHIP,
          bytesPerSecond: 128 * 1024,
          digestMs: 1200,
          pieceBytes: 16 * 1024,
        }),
      ),
      Effect.map(({ session }) => session),
    ),
})

export const fakeAdapters: Adapters = (() => {
  const mode = new URLSearchParams(location.search).get('fake')
  const withReset = mode === 'reset' || mode === 'fail'
  const failuresLeft = Ref.makeUnsafe(mode === 'fail' ? 1 : 0)
  return {
    serial: {
      choose:
        mode === 'refuse'
          ? Effect.fail(
              new ChooserFailed({
                reason:
                  'That port has no USB ids, so it cannot be found again after a reset.',
              }),
            )
          : Effect.succeed(identity),
      acquire: ({ waitMs }) =>
        Effect.sleep(Duration.millis(waitMs === 0 ? 0 : 800)).pipe(
          Effect.andThen(
            Ref.modify(failuresLeft, left =>
              left > 0 && waitMs > 0 ? [true, left - 1] : [false, left],
            ),
          ),
          Effect.flatMap(fails =>
            fails
              ? Effect.fail(new ReacquireTimeout())
              : fakeSerialLink(withReset),
          ),
        ),
      granted: () =>
        Effect.sleep(Duration.millis(300)).pipe(
          Effect.as<SerialPort>(new GrantedPort()),
        ),
    },
    flasher: fakeFlasher(demoFlash()),
    usb: { request: Effect.succeed(descriptor) },
    relay: Option.some({ dial: () => fakeRelayLink(mode === 'flash') }),
  }
})()
