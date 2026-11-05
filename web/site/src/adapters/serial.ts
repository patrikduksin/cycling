import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import { SerialIdentity } from '@cycling/protocol'

import {
  ChooserCancelled,
  ChooserFailed,
  ReacquireTimeout,
  type SerialAdapter,
  SerialLinkLost,
  type SerialLink,
  SerialOpenRefused,
} from '../links.ts'
import { REACQUIRE } from '../policy.ts'

/** Chrome's default receive buffer is 255 bytes; a flash burst would overrun it. */
const RECEIVE_BUFFER_BYTES = 64 * 1024

const errorText = (error: unknown): string =>
  error instanceof Error ? error.message : 'Unknown error'

const identityOf = (port: SerialPort): Option.Option<SerialIdentity> => {
  const { usbVendorId, usbProductId } = port.getInfo()
  return Schema.decodeUnknownOption(SerialIdentity)({
    vendorId: usbVendorId,
    productId: usbProductId,
  })
}

const matches = (port: SerialPort, identity: SerialIdentity): boolean =>
  Option.exists(
    identityOf(port),
    found =>
      found.vendorId === identity.vendorId &&
      found.productId === identity.productId,
  )

const requestPort: Effect.Effect<SerialPort, ChooserCancelled | ChooserFailed> =
  Effect.suspend(() => {
    if (import.meta.env.DEV) {
      console.info(
        `[remote] userActivation.isActive at requestPort: ${navigator.userActivation.isActive}`,
      )
    }
    return Effect.tryPromise({
      try: () => navigator.serial.requestPort(),
      catch: error =>
        error instanceof DOMException && error.name === 'NotFoundError'
          ? new ChooserCancelled()
          : new ChooserFailed({ reason: errorText(error) }),
    })
  })

const ignoreFailure = (attempt: () => Promise<unknown>): Effect.Effect<void> =>
  Effect.tryPromise(attempt).pipe(Effect.ignore)

const linkOf = (
  reader: ReadableStreamDefaultReader<Uint8Array>,
  writer: WritableStreamDefaultWriter<Uint8Array>,
): SerialLink => ({
  input: Stream.unfold(undefined, () =>
    Effect.tryPromise({
      try: () => reader.read(),
      catch: () => new SerialLinkLost(),
    }).pipe(
      Effect.map(({ done, value }) =>
        done || value === undefined ? undefined : ([value, undefined] as const),
      ),
    ),
  ),
  write: bytes =>
    Effect.tryPromise({
      try: () => writer.write(bytes),
      catch: () => new SerialLinkLost(),
    }),
})

class PortNotReady extends Schema.TaggedError<
  PortNotReady,
  { readonly brand: unique symbol }
>()('PortNotReady', { reason: Schema.String }) {}

/** Opens the port and takes both streams. Closing the Scope releases them and closes the port. */
const open = (
  port: SerialPort,
  baud: number,
): Effect.Effect<SerialLink, PortNotReady, Scope.Scope> =>
  Effect.acquireRelease(
    Effect.tryPromise({
      try: () =>
        port.open({ baudRate: baud, bufferSize: RECEIVE_BUFFER_BYTES }),
      catch: error => new PortNotReady({ reason: errorText(error) }),
    }).pipe(
      Effect.flatMap(() =>
        port.readable === null || port.writable === null
          ? Effect.fail(
              new PortNotReady({ reason: 'The port has no streams.' }),
            )
          : Effect.succeed({
              reader: port.readable.getReader(),
              writer: port.writable.getWriter(),
            }),
      ),
    ),
    ({ reader, writer }) =>
      Effect.gen(function* () {
        yield* ignoreFailure(() => reader.cancel())
        yield* Effect.ignore(Effect.try(() => reader.releaseLock()))
        yield* Effect.ignore(Effect.try(() => writer.releaseLock()))
        yield* ignoreFailure(() => port.close())
      }),
  ).pipe(Effect.map(({ reader, writer }) => linkOf(reader, writer)))

const getPorts = Effect.tryPromise({
  try: () => navigator.serial.getPorts(),
  catch: error => new PortNotReady({ reason: errorText(error) }),
})

/**
 * Granted ports worth opening for `identity`, best first: the one the visitor
 * picked when it is among them, then the matching ports nobody has open.
 */
const candidates = (options: {
  readonly ports: ReadonlyArray<SerialPort>
  readonly identity: SerialIdentity
  readonly picked: Option.Option<SerialPort>
}): ReadonlyArray<SerialPort> => {
  const matching = options.ports.filter(port => matches(port, options.identity))
  const unopened = matching.filter(port => port.readable === null)
  return Option.match(
    Option.filter(options.picked, port => matching.includes(port)),
    {
      onNone: () => unopened,
      onSome: picked => [picked, ...unopened.filter(port => port !== picked)],
    },
  )
}

const connectEvent = Effect.callback<void>(resume => {
  const onConnect = () => resume(Effect.void)
  navigator.serial.addEventListener('connect', onConnect, { once: true })
  return Effect.sync(() =>
    navigator.serial.removeEventListener('connect', onConnect),
  )
})

const nextChance = Effect.raceFirst(
  Effect.sleep(Duration.millis(REACQUIRE.pollMs)),
  connectEvent,
)

/**
 * Web Serial on this page. It keeps the SerialPort the visitor picked: two
 * boards with the same ids are told apart by object, and Chrome may hand the
 * same object back after a USB reset re-enumerates the device.
 */
export const makeWebSerial = (): SerialAdapter => {
  const picked = Ref.makeUnsafe(Option.none<SerialPort>())

  const openGranted = (identity: SerialIdentity, baud: number) =>
    Effect.all({ ports: getPorts, picked: Ref.get(picked) }).pipe(
      Effect.map(found => candidates({ ...found, identity })),
      Effect.flatMap(ports =>
        ports.length === 0
          ? Effect.fail(
              new PortNotReady({ reason: 'The device is not connected.' }),
            )
          : Effect.firstSuccessOf(ports.map(port => open(port, baud))),
      ),
    )

  const waitForPort = (
    identity: SerialIdentity,
    baud: number,
  ): Effect.Effect<SerialLink, never, Scope.Scope> =>
    openGranted(identity, baud).pipe(
      Effect.catchTag('PortNotReady', () =>
        nextChance.pipe(Effect.andThen(() => waitForPort(identity, baud))),
      ),
    )

  return {
    choose: requestPort.pipe(
      Effect.flatMap(port =>
        Effect.fromOption(identityOf(port)).pipe(
          Effect.mapError(
            () =>
              new ChooserFailed({
                reason:
                  'That port has no USB ids, so it cannot be found again after a reset.',
              }),
          ),
          Effect.tap(() => Ref.set(picked, Option.some(port))),
        ),
      ),
    ),
    acquire: ({ identity, baud, waitMs }) =>
      waitMs === 0
        ? openGranted(identity, baud).pipe(
            Effect.mapError(({ reason }) => new SerialOpenRefused({ reason })),
          )
        : waitForPort(identity, baud).pipe(
            Effect.timeoutOrElse({
              duration: Duration.millis(waitMs),
              orElse: () => Effect.fail(new ReacquireTimeout()),
            }),
          ),
  }
}
