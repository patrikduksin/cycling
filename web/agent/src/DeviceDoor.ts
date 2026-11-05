import * as Clock from 'effect/Clock'
import * as Effect from 'effect/Effect'
import * as FileSystem from 'effect/FileSystem'
import * as Match from 'effect/Match'
import * as Redacted from 'effect/Redacted'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'

import { Region } from '@cycling/protocol'

import { leaseFlasher, type Flasher } from './Flasher.ts'
import type { RemotePort } from './RemotePort.ts'
import { describeDoorOp, describeFlashError } from './Status.ts'
import type { FlashError, LocalResourceError } from './errors.ts'
import { serve } from './sockets.ts'

const AbsolutePath = Schema.String.check(Schema.isPattern(/^\//))
const Address = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0))

/**
 * One request line on the device socket, decoded strictly. A path names a
 * file on this machine that the agent reads or writes on the caller's behalf;
 * the socket lives in the 0700 session directory, so only this user can ask.
 */
export const DoorRequest = Schema.Union([
  Schema.Struct({ op: Schema.Literal('enter') }),
  Schema.Struct({
    op: Schema.Literal('read'),
    address: Address,
    size: Schema.Int,
    path: AbsolutePath,
  }),
  Schema.Struct({
    op: Schema.Literal('write'),
    address: Address,
    path: AbsolutePath,
  }),
  Schema.Struct({
    op: Schema.Literal('verify'),
    address: Address,
    path: AbsolutePath,
  }),
  Schema.Struct({
    op: Schema.Literal('reboot'),
    seconds: Schema.Finite.check(Schema.isBetween({ minimum: 0, maximum: 60 })),
    path: AbsolutePath,
  }),
])
export type DoorRequest = typeof DoorRequest.Type

const decodeRequest = Schema.decodeUnknownEffect(
  Schema.fromJsonString(DoorRequest),
  { onExcessProperty: 'error' },
)
const decodeRegion = Schema.decodeUnknownEffect(Region)

type Reply =
  | { readonly ok: Readonly<Record<string, unknown>> }
  | {
      readonly error: { readonly reason: string; readonly detail: string }
    }

const errorReply = (reason: string, detail: string): Reply => ({
  error: { reason, detail },
})

/** The wire's error vocabulary for a FlashError: OpFailed keeps the page's reason. */
const reasonOf = (error: FlashError): string =>
  Match.valueTags(error, {
    OpFailed: ({ reason }) => reason,
    TransferBroken: () => 'transferBroken',
    Unreachable: () => 'unreachable',
    OpStalled: () => 'stalled',
    BadRequest: () => 'badRequest',
  })

const answer = (
  flasher: Flasher,
  request: DoorRequest,
): Effect.Effect<Reply, FlashError> =>
  Match.value(request).pipe(
    Match.withReturnType<Effect.Effect<Reply, FlashError>>(),
    Match.discriminatorsExhaustive('op')({
      enter: () =>
        flasher.enter.pipe(
          Effect.map(chip => ({
            ok: {
              mac: Redacted.value(chip.mac),
              secure_boot: chip.secureBoot,
              flash_encryption: chip.flashEncryption,
            },
          })),
        ),
      read: ({ address, size, path }) =>
        decodeRegion({ address, size }).pipe(
          Effect.matchEffect({
            onFailure: () =>
              Effect.succeed(
                errorReply('badRequest', 'not a region of the 16 MiB flash'),
              ),
            onSuccess: region =>
              flasher
                .read(region, path)
                .pipe(Effect.map(md5 => ({ ok: { md5 } }))),
          }),
        ),
      write: ({ address, path }) =>
        flasher.write(address, path).pipe(Effect.map(md5 => ({ ok: { md5 } }))),
      verify: ({ address, path }) =>
        flasher
          .verify(address, path)
          .pipe(Effect.map(matched => ({ ok: { matched } }))),
      reboot: ({ seconds, path }) =>
        flasher
          .reboot(seconds, path)
          .pipe(Effect.map(captured => ({ ok: { captured } }))),
    }),
  )

const encoder = new TextEncoder()
const line = (reply: Reply) => encoder.encode(`${JSON.stringify(reply)}\n`)

/**
 * `<dir>/<CODE>/device.sock`, the child's `CYCLING_FLASHER`: JSON lines, one
 * request then one reply. One connection is one lease, released when the
 * connection closes. A second connection while one is open is answered
 * `busy` and closed; `usb.lock` already keeps devtools to one device task.
 * Each op prints one notice, never the MAC.
 */
export const serveDeviceDoor = (options: {
  readonly path: string
  readonly port: RemotePort
  readonly notice: (text: string) => Effect.Effect<void>
}): Effect.Effect<
  void,
  LocalResourceError,
  Scope.Scope | FileSystem.FileSystem
> =>
  Effect.gen(function* () {
    const { path, port, notice } = options
    const fs = yield* FileSystem.FileSystem
    const remove = Effect.ignore(fs.remove(path, { force: true }))
    yield* Effect.acquireRelease(remove, () => remove)
    const held = yield* Ref.make(false)

    const timed = (request: DoorRequest, flasher: Flasher) =>
      Effect.gen(function* () {
        const started = yield* Clock.currentTimeMillis
        const reply = yield* answer(flasher, request).pipe(
          Effect.catch(error =>
            notice(describeFlashError(request, error)).pipe(
              Effect.as(errorReply(reasonOf(error), errorDetail(error))),
            ),
          ),
        )
        if ('ok' in reply) {
          const ms = (yield* Clock.currentTimeMillis) - started
          yield* notice(describeDoorOp(request, ms))
        }
        return reply
      })

    yield* serve({
      listen: { path },
      resource: 'deviceDoor',
      handle: client =>
        Effect.gen(function* () {
          if (yield* Ref.getAndSet(held, true)) {
            yield* client.write(
              line(errorReply('busy', 'another device task holds the device')),
            )
            return
          }
          yield* Effect.addFinalizer(() => Ref.set(held, false))
          const flasher = yield* leaseFlasher(port)
          yield* client.incoming.pipe(
            Stream.decodeText,
            Stream.splitLines,
            Stream.filter(text => text.trim().length > 0),
            Stream.runForEach(text =>
              decodeRequest(text).pipe(
                Effect.matchEffect({
                  onFailure: () =>
                    Effect.succeed(
                      errorReply('badRequest', 'not a device request'),
                    ),
                  onSuccess: request => timed(request, flasher),
                }),
                Effect.flatMap(reply => client.write(line(reply))),
              ),
            ),
          )
        }).pipe(Effect.provideService(FileSystem.FileSystem, fs)),
    })
  })

const errorDetail = (error: FlashError): string =>
  Match.valueTags(error, {
    OpFailed: ({ detail }) => detail,
    TransferBroken: ({ detail }) => detail,
    Unreachable: ({ seconds }) =>
      `the visitor's page was unreachable for ${seconds} s`,
    OpStalled: ({ ms }) => `no progress from the page for ${ms / 1000} s`,
    BadRequest: ({ detail }) => detail,
  })
