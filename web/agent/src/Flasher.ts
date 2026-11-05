import * as Effect from 'effect/Effect'
import * as FileSystem from 'effect/FileSystem'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as Redacted from 'effect/Redacted'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import { createHash, randomBytes } from 'node:crypto'

import {
  type Chip,
  Lease,
  type Mac,
  Md5,
  type Op,
  type OpOutcome,
  Region,
} from '@cycling/protocol'

import { firstMatch, type RemotePort } from './RemotePort.ts'
import {
  BadRequest,
  type FlashError,
  OpFailed,
  type OpTransportError,
  TransferBroken,
  Unreachable,
} from './errors.ts'

/** How long a reboot waits for the device to come back as the console. */
export const REBOOT_REAPPEAR_MS = 10_000
/** How long an op waits for a dropped relay or an absent visitor before it gives up. */
export const LINK_WAIT_MS = 30_000
/** Resends of an idempotent op after a relay drop or a broken transfer. */
export const OP_RETRIES = 2
const RELEASE_WAIT_MS = 3000

export interface EnteredChip {
  readonly mac: Redacted.Redacted<Mac>
  readonly secureBoot: boolean
  readonly flashEncryption: boolean
}

/**
 * One lease on the visitor's device: the agent's half of python's Flasher
 * protocol, verb for verb. Files, integrity and retries live here, so the
 * page only moves bytes next to the chip and Python only decides.
 */
export interface Flasher {
  readonly enter: Effect.Effect<EnteredChip, FlashError>
  /** `path` holds exactly `region.size` bytes whose MD5 is the chip's own. */
  readonly read: (
    region: Region,
    path: string,
  ) => Effect.Effect<Md5, FlashError>
  /** Written, and the chip's digest of the region equals the file's. */
  readonly write: (
    address: number,
    path: string,
  ) => Effect.Effect<Md5, FlashError>
  /** Whether the chip's MD5 of the file's length at `address` equals the file's. */
  readonly verify: (
    address: number,
    path: string,
  ) => Effect.Effect<boolean, FlashError>
  /**
   * Watchdog reset into the selected app, then `seconds` of console output
   * into `path`. False when the device did not come back as the console in
   * `REBOOT_REAPPEAR_MS` (stock firmware may drop USB).
   */
  readonly reboot: (
    seconds: number,
    path: string,
  ) => Effect.Effect<boolean, FlashError>
}

export const md5Of = (bytes: Uint8Array): Md5 =>
  Md5.make(createHash('md5').update(bytes).digest('hex'))

const mintLease = (): Lease => Lease.make(randomBytes(12).toString('base64url'))

const decodeRegion = Schema.decodeUnknownEffect(Region)

const regionOf = (address: number, size: number) =>
  decodeRegion({ address, size }).pipe(
    Effect.mapError(
      () =>
        new BadRequest({
          detail: `0x${address.toString(16)}+0x${size.toString(16)} is not a region of the 16 MiB flash`,
        }),
    ),
  )

interface Answer {
  readonly outcome: OpOutcome
  readonly data: ReadonlyArray<Uint8Array>
  readonly received: number
}

interface Collected extends Omit<Answer, 'outcome'> {
  readonly outcome: Option.Option<OpOutcome>
}

const unexpected = (outcome: OpOutcome) =>
  new TransferBroken({ detail: `unexpected answer ${outcome._tag}` })

/** What a Failed answer means here; any other outcome is handed back. */
const settled = (answer: Answer): Effect.Effect<Answer, OpFailed> =>
  answer.outcome._tag === 'Failed'
    ? Effect.fail(
        new OpFailed({
          reason: answer.outcome.reason,
          detail: answer.outcome.detail,
        }),
      )
    : Effect.succeed(answer)

const concat = (chunks: ReadonlyArray<Uint8Array>, size: number) => {
  const out = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) {
    out.set(chunk, offset)
    offset += chunk.byteLength
  }
  return out
}

/** Released when the scope closes: a Python process that exits, crashes or is interrupted gives the device back. */
export const leaseFlasher = (
  port: RemotePort,
): Effect.Effect<Flasher, never, Scope.Scope | FileSystem.FileSystem> =>
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem
    const lease = mintLease()

    /** Collects one op's answer; read data must arrive in order with no hole. */
    const once = (
      op: Op,
      upload?: Uint8Array,
    ): Effect.Effect<Answer, OpTransportError | TransferBroken | OpFailed> =>
      port.op(op, upload).pipe(
        Stream.runFoldEffect(
          (): Collected => ({ outcome: Option.none(), data: [], received: 0 }),
          (collected, event): Effect.Effect<Collected, TransferBroken> =>
            Match.value(event).pipe(
              Match.tagsExhaustive({
                Progress: () => Effect.succeed(collected),
                Result: ({ outcome }) =>
                  Effect.succeed({
                    ...collected,
                    outcome: Option.some(outcome),
                  }),
                Data: ({ offset, bytes }) =>
                  offset === collected.received
                    ? Effect.succeed({
                        ...collected,
                        data: [...collected.data, bytes],
                        received: collected.received + bytes.byteLength,
                      })
                    : Effect.fail(
                        new TransferBroken({
                          detail: `read data jumped from ${collected.received} to ${offset}`,
                        }),
                      ),
              }),
            ),
        ),
        Effect.flatMap(
          ({
            outcome,
            data,
            received,
          }): Effect.Effect<Answer, TransferBroken | OpFailed> =>
            Option.isNone(outcome)
              ? Effect.fail(new TransferBroken({ detail: 'no answer' }))
              : settled({ outcome: outcome.value, data, received }),
        ),
      )

    const joined = firstMatch(port.status, status =>
      status.link._tag === 'Joined' && status.link.browserPresent
        ? Option.some(true)
        : status.link._tag === 'Over'
          ? Option.some(false)
          : Option.none(),
    ).pipe(
      Effect.timeoutOrElse({
        duration: LINK_WAIT_MS,
        orElse: () => Effect.succeed(false),
      }),
      Effect.flatMap(back =>
        back
          ? Effect.void
          : Effect.fail(new Unreachable({ seconds: LINK_WAIT_MS / 1000 })),
      ),
    )

    /**
     * Every op but Reboot is idempotent under one lease, so a relay drop or
     * a broken transfer resends it once the visitor is reachable again.
     */
    const retried = <A>(
      run: Effect.Effect<
        A,
        OpTransportError | TransferBroken | OpFailed | BadRequest
      >,
      retries = OP_RETRIES,
    ): Effect.Effect<A, FlashError> =>
      run.pipe(
        Effect.catchTags({
          RelayDown: () =>
            retries > 0
              ? joined.pipe(Effect.andThen(retried(run, retries - 1)))
              : Effect.fail(new Unreachable({ seconds: LINK_WAIT_MS / 1000 })),
          TransferBroken: error =>
            retries > 0
              ? joined.pipe(Effect.andThen(retried(run, retries - 1)))
              : Effect.fail(error),
        }),
      )

    const readFile = (path: string) =>
      fs.readFile(path).pipe(
        Effect.mapError(
          error =>
            new BadRequest({
              detail: `cannot read ${path}: ${error.message}`,
            }),
        ),
      )

    const writeFile = (path: string, bytes: Uint8Array) =>
      fs.writeFile(path, bytes).pipe(
        Effect.mapError(
          error =>
            new BadRequest({
              detail: `cannot write ${path}: ${error.message}`,
            }),
        ),
      )

    const digest = (region: Region) =>
      once({ _tag: 'Digest', lease, region }).pipe(
        Effect.flatMap(({ outcome }) =>
          outcome._tag === 'Digest'
            ? Effect.succeed(outcome.md5)
            : Effect.fail(unexpected(outcome)),
        ),
      )

    yield* Effect.addFinalizer(() =>
      once({ _tag: 'Release', lease }).pipe(
        Effect.timeout(RELEASE_WAIT_MS),
        Effect.ignore,
      ),
    )

    const enter = retried(
      once({ _tag: 'Enter', lease }).pipe(
        Effect.flatMap(({ outcome }) =>
          outcome._tag === 'Entered'
            ? Effect.succeed(chipOf(outcome.chip))
            : Effect.fail(unexpected(outcome)),
        ),
      ),
    )

    const read = (region: Region, path: string) =>
      retried(
        Effect.gen(function* () {
          const answer = yield* once({ _tag: 'Read', lease, region })
          if (answer.outcome._tag !== 'Read') {
            return yield* unexpected(answer.outcome)
          }
          if (answer.received !== region.size) {
            return yield* new TransferBroken({
              detail: `read ${answer.received} of ${region.size} bytes`,
            })
          }
          const bytes = concat(answer.data, answer.received)
          if (md5Of(bytes) !== answer.outcome.md5) {
            return yield* new TransferBroken({
              detail: "the read bytes are not the chip's",
            })
          }
          yield* writeFile(path, bytes)
          return answer.outcome.md5
        }),
      )

    const write = (address: number, path: string) =>
      retried(
        Effect.gen(function* () {
          const bytes = yield* readFile(path)
          const region = yield* regionOf(address, bytes.byteLength)
          const md5 = md5Of(bytes)
          const { outcome } = yield* once(
            { _tag: 'Write', lease, region, md5 },
            bytes,
          )
          if (outcome._tag !== 'Written') return yield* unexpected(outcome)
          if (outcome.md5 !== md5) {
            return yield* new TransferBroken({
              detail: "the chip's digest after the write is not the file's",
            })
          }
          return md5
        }),
      )

    const verify = (address: number, path: string) =>
      retried(
        Effect.gen(function* () {
          const bytes = yield* readFile(path)
          const region = yield* regionOf(address, bytes.byteLength)
          return (yield* digest(region)) === md5Of(bytes)
        }),
      )

    const reboot = (seconds: number, path: string) =>
      Effect.scoped(
        Effect.gen(function* () {
          const console = yield* port.listen
          const { outcome } = yield* once({ _tag: 'Reboot', lease })
          if (outcome._tag !== 'Rebooting') return yield* unexpected(outcome)
          const back = yield* firstMatch(port.status, status =>
            Option.exists(status.port, ({ state }) => state._tag === 'Open')
              ? Option.some(true)
              : Option.none(),
          ).pipe(
            Effect.timeoutOrElse({
              duration: REBOOT_REAPPEAR_MS,
              orElse: () => Effect.succeed(false),
            }),
          )
          if (!back) return false
          const chunks = yield* console.pipe(
            Stream.interruptWhen(Effect.sleep(seconds * 1000)),
            Stream.runCollect,
          )
          const size = chunks.reduce((n, chunk) => n + chunk.byteLength, 0)
          yield* writeFile(path, concat(chunks, size))
          return true
        }),
      ).pipe(
        Effect.catchTag('RelayDown', () =>
          Effect.fail(new Unreachable({ seconds: 0 })),
        ),
      )

    return { enter, read, write, verify, reboot } satisfies Flasher
  })

const chipOf = (chip: Chip): EnteredChip => ({
  mac: Redacted.make(chip.mac),
  secureBoot: chip.secureBoot,
  flashEncryption: chip.flashEncryption,
})
