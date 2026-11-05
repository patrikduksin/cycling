import * as Cause from 'effect/Cause'
import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import {
  CloseCode,
  type RelayEvent,
  SerialIdentity,
  type ToBrowser,
  UsbDescriptor,
} from '@cycling/protocol'

import {
  type Adapters,
  ChooserFailed,
  type RelayLink,
  ReacquireTimeout,
  SerialLinkLost,
  type SerialLink,
} from '../links.ts'

/**
 * A device and a relay that exist only in the dev server (`?fake`), so the
 * Live screen and the byte monitor can be seen without hardware or a Worker.
 * `?fake=reset` also has the device reset itself after about five seconds,
 * which walks the port through Lost and Reacquiring.
 * `?fake=fail` does the same, but the device never comes back, so the page
 * shows the failed port and "Choose device again". Disconnect closes the
 * fake socket the way the relay does, with `visitorDisconnected`.
 * `?fake=refuse` has the chooser fail, so Idle shows its notice.
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

const fakeRelayLink = (): Effect.Effect<RelayLink, never, Scope.Scope> =>
  Effect.gen(function* () {
    const inbound = yield* Queue.unbounded<RelayEvent<ToBrowser>, Cause.Done>()
    yield* Effect.forkScoped(
      Effect.gen(function* () {
        yield* Queue.offer(inbound, control(welcome(false)))
        yield* Effect.sleep(Duration.millis(1500))
        yield* Queue.offer(
          inbound,
          control({ _tag: 'PeerPresence', role: 'agent', present: true }),
        )
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
        item._tag !== 'Bye'
          ? Effect.succeed(true)
          : Queue.offer(inbound, {
              _tag: 'Closed',
              code: CloseCode.visitorDisconnected,
            }).pipe(Effect.andThen(Queue.end(inbound))),
    }
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
    },
    usb: { request: Effect.succeed(descriptor) },
    relay: Option.some({ dial: fakeRelayLink }),
  }
})()
