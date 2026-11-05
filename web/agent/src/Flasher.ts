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
  type Assembly,
  type Chip,
  Lease,
  MAX_FLASH_BYTES,
  type Mac,
  Md5,
  type OpFailure,
  type OpOutcome,
  Region,
  type RequestId,
  assemble,
  assembled,
  emptyAssembly,
} from '@cycling/protocol'

import { firstMatch, type OpRequest, type RemotePort } from './RemotePort.ts'
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
/** Resends of an idempotent op: after a relay drop, a stall, broken read data or a refusal in `RESENT`. */
export const OP_RETRIES = 3
const RELEASE_WAIT_MS = 3000
const AWAY = `the visitor's page was unreachable for ${LINK_WAIT_MS / 1000} s`

/** The page's chip description with the MAC kept out of logs and traces. */
export interface EnteredChip extends Omit<Chip, 'mac'> {
  readonly mac: Redacted.Redacted<Mac>
}

/**
 * One lease on the visitor's device: the agent's half of python's Flasher
 * protocol, verb for verb. Files, integrity and retries live here, so the
 * page only moves bytes next to the chip and Python only decides.
 */
export interface Flasher {
  readonly enter: Effect.Effect<EnteredChip, FlashError>
  /** `path` holds exactly `size` bytes whose MD5 is the chip's own. */
  readonly read: (
    address: number,
    size: number,
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

/** What the page answered to one op, with the read data that came before the answer. */
interface Answer {
  readonly outcome: OpOutcome
  readonly data: Assembly
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

/** Refusals the page makes before flash changed, or after its own retries: another try may pass. */
const RESENT: ReadonlySet<OpFailure> = new Set([
  'transferGap',
  'payloadMismatch',
  'digestMismatch',
])

type OpError = OpTransportError | TransferBroken | OpFailed | BadRequest

/** Released when the scope closes: a Python process that exits, crashes or is interrupted gives the device back. */
export const leaseFlasher = (
  port: RemotePort,
): Effect.Effect<Flasher, never, Scope.Scope | FileSystem.FileSystem> =>
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem
    const lease = mintLease()

    /** Collects one op's answer; read data must arrive in order with no hole. */
    const once = (
      id: RequestId,
      request: OpRequest,
    ): Effect.Effect<Answer, OpTransportError | TransferBroken | OpFailed> =>
      port.op(id, request).pipe(
        Stream.runFoldEffect(
          () => ({
            outcome: Option.none<OpOutcome>(),
            data: emptyAssembly(MAX_FLASH_BYTES),
          }),
          (collected, event) =>
            Match.value(event).pipe(
              Match.withReturnType<
                Effect.Effect<typeof collected, TransferBroken>
              >(),
              Match.tagsExhaustive({
                Progress: () => Effect.succeed(collected),
                Result: ({ outcome }) =>
                  Effect.succeed({
                    ...collected,
                    outcome: Option.some(outcome),
                  }),
                Data: frame =>
                  Option.match(assemble(collected.data, frame), {
                    onNone: () =>
                      Effect.fail(
                        new TransferBroken({
                          detail: `read data jumped from ${collected.data.received} to ${frame.offset}`,
                        }),
                      ),
                    onSome: data => Effect.succeed({ ...collected, data }),
                  }),
              }),
            ),
        ),
        Effect.flatMap(
          ({
            outcome,
            data,
          }): Effect.Effect<Answer, TransferBroken | OpFailed> =>
            Option.match(outcome, {
              onNone: () =>
                Effect.fail(new TransferBroken({ detail: 'no answer' })),
              onSome: outcome => settled({ outcome, data }),
            }),
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
        back ? Effect.void : Effect.fail(new Unreachable({ detail: AWAY })),
      ),
    )

    /**
     * Every op but Reboot is idempotent under one lease. One whose answer
     * never came (a relay drop, `OP_STALL_MS` of silence) is resent under its own id,
     * so the page joins it while it runs or answers it again; one whose
     * answer was unusable (broken read data, a refusal in `RESENT`) is tried
     * again under a new id. Each waits for the visitor to be reachable.
     */
    const retried = <A>(
      run: (id: RequestId) => Effect.Effect<A, OpError>,
    ): Effect.Effect<A, FlashError> => {
      const attempt = (
        id: RequestId,
        left: number,
      ): Effect.Effect<A, FlashError> => {
        const again = <E>(error: E, next: Effect.Effect<RequestId>) =>
          left > 0
            ? joined.pipe(
                Effect.andThen(next),
                Effect.flatMap(nextId => attempt(nextId, left - 1)),
              )
            : Effect.fail(error)
        return run(id).pipe(
          Effect.catchTags({
            RelayDown: () =>
              again(new Unreachable({ detail: AWAY }), Effect.succeed(id)),
            OpStalled: error => again(error, Effect.succeed(id)),
            TransferBroken: error => again(error, port.nextId),
            OpFailed: error =>
              RESENT.has(error.reason)
                ? again(error, port.nextId)
                : Effect.fail(error),
          }),
        )
      }
      return Effect.flatMap(port.nextId, id => attempt(id, OP_RETRIES))
    }

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

    yield* Effect.addFinalizer(() =>
      port.nextId.pipe(
        Effect.flatMap(id => once(id, { op: { _tag: 'Release', lease } })),
        Effect.timeout(RELEASE_WAIT_MS),
        Effect.ignore,
      ),
    )

    const enter = retried(id =>
      once(id, { op: { _tag: 'Enter', lease } }).pipe(
        Effect.flatMap(({ outcome }) =>
          outcome._tag === 'Entered'
            ? Effect.succeed(chipOf(outcome.chip))
            : Effect.fail(unexpected(outcome)),
        ),
      ),
    )

    const read = (address: number, size: number, path: string) =>
      Effect.flatMap(regionOf(address, size), region =>
        retried(id =>
          Effect.gen(function* () {
            const { outcome, data } = yield* once(id, {
              op: { _tag: 'Read', lease, region },
            })
            if (outcome._tag !== 'Read') {
              return yield* unexpected(outcome)
            }
            if (data.received !== region.size) {
              return yield* new TransferBroken({
                detail: `read ${data.received} of ${region.size} bytes`,
              })
            }
            const bytes = assembled(data)
            if (md5Of(bytes) !== outcome.md5) {
              return yield* new TransferBroken({
                detail: "the read bytes are not the chip's",
              })
            }
            yield* writeFile(path, bytes)
            return outcome.md5
          }),
        ),
      )

    const write = (address: number, path: string) =>
      Effect.gen(function* () {
        const payload = yield* readFile(path)
        const region = yield* regionOf(address, payload.byteLength)
        const md5 = md5Of(payload)
        return yield* retried(id =>
          once(id, { op: { _tag: 'Write', lease, region, md5 }, payload }).pipe(
            Effect.flatMap(({ outcome }) =>
              outcome._tag !== 'Written'
                ? Effect.fail(unexpected(outcome))
                : outcome.md5 !== md5
                  ? Effect.fail(
                      new TransferBroken({
                        detail:
                          "the chip's digest after the write is not the file's",
                      }),
                    )
                  : Effect.succeed(md5),
            ),
          ),
        )
      })

    const verify = (address: number, path: string) =>
      Effect.gen(function* () {
        const bytes = yield* readFile(path)
        const region = yield* regionOf(address, bytes.byteLength)
        const chip = yield* retried(id =>
          once(id, { op: { _tag: 'Digest', lease, region } }).pipe(
            Effect.flatMap(({ outcome }) =>
              outcome._tag === 'Digest'
                ? Effect.succeed(outcome.md5)
                : Effect.fail(unexpected(outcome)),
            ),
          ),
        )
        return chip === md5Of(bytes)
      })

    const reboot = (seconds: number, path: string) =>
      Effect.scoped(
        Effect.gen(function* () {
          const console = yield* port.listen
          const { outcome } = yield* once(yield* port.nextId, {
            op: { _tag: 'Reboot', lease },
          })
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
          yield* writeFile(path, Buffer.concat(chunks))
          return true
        }),
      ).pipe(
        Effect.catchTag('RelayDown', () =>
          Effect.fail(
            new Unreachable({
              detail: 'the relay dropped while the device was asked to reboot',
            }),
          ),
        ),
      )

    return { enter, read, write, verify, reboot } satisfies Flasher
  })

const chipOf = (chip: Chip): EnteredChip => ({
  ...chip,
  mac: Redacted.make(chip.mac),
})
