import type * as Cause from 'effect/Cause'
import * as Deferred from 'effect/Deferred'
import * as Effect from 'effect/Effect'
import * as Match from 'effect/Match'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import type { IEspLoaderTerminal } from 'esptool-js'
import SparkMD5 from 'spark-md5'

import { DISABLE_WATCHDOGS, type RegStep, WATCHDOG_RESET } from './watchdog.ts'

/**
 * `usbReset` runs the USB-Serial-JTAG reset into the ROM. `noReset` syncs
 * with a chip that is already in download mode, for the second try after the
 * first entry lost the port to a re-enumeration.
 */
export type Entry = 'usbReset' | 'noReset'

export const FlasherFailure = Schema.Literals([
  /** connect, sync, security info, the watchdog writes or the stub failed. */
  'entryFailed',
  /** esptool-js failed during an op: a timeout, a bad reply, a flash error. */
  'chipError',
  /** A read piece kept disagreeing with the chip's own MD5, or a write's did. */
  'digestMismatch',
  /** The USB device went away. */
  'portLost',
])
export type FlasherFailure = typeof FlasherFailure.Type

export class FlasherError extends Schema.TaggedError<
  FlasherError,
  { readonly brand: unique symbol }
>()('FlasherError', {
  reason: FlasherFailure,
  /** esptool-js's words, MAC-shaped text redacted, at most 200 characters. */
  detail: Schema.String,
}) {}

export interface Chip {
  /** The eFuse MAC as 12 lowercase hex. Never logged, never shown. */
  readonly mac: string
  readonly secureBoot: boolean
  /** Odd popcount of flash_crypt_cnt, as python esptool computes it. */
  readonly flashEncryption: boolean
}

export interface Region {
  readonly address: number
  readonly size: number
}

export type ReadEvent =
  /** Bytes of the region read and verified so far. */
  | { readonly _tag: 'Progress'; readonly done: number }
  /** A verified piece, in order; `offset` is relative to the region's address. */
  | {
      readonly _tag: 'Piece'
      readonly offset: number
      readonly bytes: Uint8Array
    }

/**
 * One ROM-and-stub session on a SerialPort, driven by one fiber at a time:
 * esptool-js flushes input before every command, so two concurrent calls
 * would corrupt each other.
 */
export interface Session {
  readonly chip: Chip
  /**
   * Stub reads in pieces of at most `READ_PIECE_BYTES`. Each piece is checked
   * against the chip's MD5 of the same range and retried up to
   * `PIECE_RETRIES` times before `digestMismatch`.
   */
  readonly read: (region: Region) => Stream.Stream<ReadEvent, FlasherError>
  /**
   * write-flash keep/keep/keep, compressed, then the chip's MD5 of the
   * written range against the data's (`digestMismatch`). Emits bytes
   * written so far.
   */
  readonly write: (
    address: number,
    data: Uint8Array,
  ) => Stream.Stream<number, FlasherError>
  /** The stub's SPI_FLASH_MD5 of the region, lowercase hex. */
  readonly digest: (region: Region) => Effect.Effect<string, FlasherError>
  /**
   * python esptool's S3 `watchdog_reset`. The reply to the last write may
   * never come; that is success. The port is lost next.
   */
  readonly reboot: Effect.Effect<void, FlasherError>
  /** What esptool-js says, line by line, MAC-shaped text redacted. Ends when the scope closes. */
  readonly log: Stream.Stream<string>
  /** Succeeds when the USB device goes away. */
  readonly lost: Effect.Effect<void>
}

/**
 * About 2 s of the chip's ~140 KB/s: an abandoned piece settles inside
 * SETTLE_MS, so a Release mid-read still gets its recovery reset, and the
 * visitor's progress moves every couple of seconds.
 */
export const READ_PIECE_BYTES = 256 * 1024
export const PIECE_RETRIES = 2

/** How long the closing scope waits for an abandoned esptool-js call, then for the recovery reset, then for disconnect. */
const SETTLE_MS = 4000
const RECOVERY_MS = 2000
const DISCONNECT_MS = 2000
const ESPRESSIF_VID = 0x303a
const USB_JTAG_SERIAL_PID = 0x1001

/** Where the session stands, which decides what closing it owes the chip. */
type Standing = 'connecting' | 'talking' | 'rebooted'

const MAC_SHAPED =
  /(?<![0-9a-f])[0-9a-f]{2}([:\- ]?)[0-9a-f]{2}(?:\1[0-9a-f]{2}){4}(?![0-9a-f])/gi

/** MAC-shaped text (12 hex digits, with or without separators) becomes `<mac>`; the result is at most 200 characters. */
export const redact = (text: string): string =>
  text.replace(MAC_SHAPED, '<mac>').slice(0, 200)

/** Lowercase hex MD5 (WebCrypto has none). */
export const md5Hex = (bytes: Uint8Array): string =>
  SparkMD5.ArrayBuffer.hash(bytes.slice().buffer)

const messageOf = (error: unknown): string =>
  error instanceof Error ? error.message : String(error)

const hex = (value: number): string => `0x${value.toString(16)}`

const popcount = (value: number): number =>
  value.toString(2).replaceAll('0', '').length

const portLost = new FlasherError({
  reason: 'portLost',
  detail: 'The USB device went away.',
})

/** Pieces of a region, offsets relative to its address. */
const piecesOf = (region: Region): ReadonlyArray<Region> =>
  Array.from(
    { length: Math.ceil(region.size / READ_PIECE_BYTES) },
    (_, index) => ({
      address: index * READ_PIECE_BYTES,
      size: Math.min(READ_PIECE_BYTES, region.size - index * READ_PIECE_BYTES),
    }),
  )

/** esptool-js writes partial lines; the log gets whole ones. */
const lineTerminal = (say: (line: string) => void): IEspLoaderTerminal => {
  let partial = ''
  const write = (text: string) => {
    const lines = (partial + text).split(/\r\n|\r|\n/)
    partial = lines.pop() ?? ''
    for (const line of lines) {
      if (line.trim() !== '') {
        say(line)
      }
    }
  }
  return { clean: () => {}, write, writeLine: text => write(`${text}\n`) }
}

/**
 * Opens the port (esptool-js's Transport does the `open`, so the caller must
 * have closed it), enters download mode, disables the S3 RTC and super
 * watchdogs when the port is USB-Serial-JTAG (303a:1001, the same test as
 * python esptool's `uses_usb_jtag_serial`), reads the security
 * facts and the MAC, and starts the stub.
 *
 * Closing the scope is the only way out. Unless `reboot` succeeded or the
 * device is gone, it makes one best-effort watchdog reset, so a stranger's
 * device is not left in download mode; then it disconnects, which closes the
 * port.
 */
export const open = (
  port: SerialPort,
  options: { readonly entry: Entry },
): Effect.Effect<Session, FlasherError, Scope.Scope> =>
  Effect.gen(function* () {
    const lines = yield* Queue.unbounded<string, Cause.Done>()
    const say = (line: string) => {
      Queue.offerUnsafe(lines, redact(line))
    }
    const gone = yield* Deferred.make<void>()
    const standing = yield* Ref.make<Standing>('connecting')

    yield* Effect.acquireRelease(
      Effect.sync(() => {
        const onDisconnect = () => {
          Deferred.doneUnsafe(gone, Effect.void)
        }
        port.addEventListener('disconnect', onDisconnect)
        return onDisconnect
      }),
      onDisconnect =>
        Effect.sync(() => port.removeEventListener('disconnect', onDisconnect)),
    )

    const esptool = yield* Effect.tryPromise({
      try: () => import('esptool-js'),
      catch: error =>
        new FlasherError({
          reason: 'entryFailed',
          detail: redact(messageOf(error)),
        }),
    })

    /** Transport.trace calls console.log unconditionally; keep it in the log. */
    class LoggedTransport extends esptool.Transport {
      override trace(message: string): void {
        say(message)
      }
    }
    const transport = new LoggedTransport(port, false)
    const loader = new esptool.ESPLoader({
      transport,
      baudrate: 115200,
      romBaudrate: 115200,
      terminal: lineTerminal(say),
      enableTracing: false,
    })

    /** The esptool-js promise last started; an interrupted one keeps running and owns the wire until it settles. */
    let inflight: Promise<unknown> = Promise.resolve()

    const call = <A>(
      reason: FlasherFailure,
      run: () => Promise<A>,
    ): Effect.Effect<A, FlasherError> =>
      Effect.tryPromise({
        try: () => {
          const promise = run()
          inflight = promise
          return promise
        },
        catch: error =>
          new FlasherError({ reason, detail: redact(messageOf(error)) }),
      }).pipe(
        Effect.catch(error =>
          Effect.flatMap(Deferred.isDone(gone), isGone =>
            Effect.fail(isGone ? portLost : error),
          ),
        ),
        Effect.raceFirst(
          Deferred.await(gone).pipe(Effect.andThen(Effect.fail(portLost))),
        ),
      )

    const step =
      (reason: FlasherFailure) =>
      (regStep: RegStep): Effect.Effect<void, FlasherError> =>
        Match.valueTags(regStep, {
          Write: ({ address, value }) =>
            call(reason, () => loader.writeReg(address, value)),
          SetBits: ({ address, bits }) =>
            call(reason, () => loader.readReg(address)).pipe(
              Effect.flatMap(value =>
                call(reason, () =>
                  loader.writeReg(address, (value | bits) >>> 0),
                ),
              ),
            ),
          Hold: ({ ms }) => Effect.sleep(ms),
        })

    const watchdogResetLastWrite = WATCHDOG_RESET.reduce(
      (last, regStep, index) => (regStep._tag === 'Write' ? index : last),
      -1,
    )
    const watchdogReset = Effect.forEach(
      WATCHDOG_RESET,
      (regStep, index) =>
        index === watchdogResetLastWrite
          ? Effect.ignore(step('chipError')(regStep))
          : step('chipError')(regStep),
      { discard: true },
    )

    const close = Effect.gen(function* () {
      const owed =
        (yield* Ref.get(standing)) === 'talking' &&
        !(yield* Deferred.isDone(gone))
      if (owed) {
        yield* Effect.promise(() => inflight.then(noop, noop)).pipe(
          Effect.timeout(SETTLE_MS),
          Effect.andThen(watchdogReset.pipe(Effect.timeout(RECOVERY_MS))),
          Effect.ignore,
        )
      }
      yield* Effect.promise(() => transport.disconnect().then(noop, noop)).pipe(
        Effect.timeout(DISCONNECT_MS),
        Effect.ignore,
      )
      yield* Queue.end(lines)
    })
    yield* Effect.addFinalizer(() => close)

    yield* call('entryFailed', () =>
      loader.connect(options.entry === 'usbReset' ? 'usb_reset' : 'no_reset'),
    )
    yield* Ref.set(standing, 'talking')
    const usb = port.getInfo()
    if (
      usb.usbVendorId === ESPRESSIF_VID &&
      usb.usbProductId === USB_JTAG_SERIAL_PID
    ) {
      yield* Effect.forEach(DISABLE_WATCHDOGS, step('entryFailed'), {
        discard: true,
      })
    }
    const security = yield* call('entryFailed', () =>
      loader.getSecurityInfo(false),
    )
    const mac = yield* call('entryFailed', () => loader.chip.readMac(loader))
    yield* call('entryFailed', () => loader.runStub())

    const chip: Chip = {
      mac: mac.replaceAll(':', '').toLowerCase(),
      secureBoot: security.parsedFlags.SECURE_BOOT_EN,
      flashEncryption: popcount(security.flashCryptCnt) % 2 === 1,
    }

    const digest = (region: Region) =>
      call('chipError', () => loader.flashMd5sum(region.address, region.size))

    const readPiece = (region: Region) =>
      Effect.gen(function* () {
        const bytes = yield* call('chipError', () =>
          loader.readFlash(region.address, region.size),
        )
        const expected = yield* digest(region)
        if (md5Hex(bytes) !== expected) {
          return yield* new FlasherError({
            reason: 'digestMismatch',
            detail: `The read of ${hex(region.address)}+${hex(region.size)} disagrees with the chip's MD5.`,
          })
        }
        return bytes
      }).pipe(
        Effect.retry({
          times: PIECE_RETRIES,
          while: error => error.reason === 'digestMismatch',
        }),
      )

    const read = (region: Region): Stream.Stream<ReadEvent, FlasherError> =>
      Stream.fromIterable(piecesOf(region)).pipe(
        Stream.mapEffect(piece =>
          readPiece({
            address: region.address + piece.address,
            size: piece.size,
          }).pipe(
            Effect.map((bytes): ReadonlyArray<ReadEvent> => [
              { _tag: 'Piece', offset: piece.address, bytes },
              { _tag: 'Progress', done: piece.address + piece.size },
            ]),
          ),
        ),
        Stream.flattenIterable,
      )

    const write = (
      address: number,
      data: Uint8Array,
    ): Stream.Stream<number, FlasherError> =>
      Stream.callback<number, FlasherError>(progress =>
        Effect.gen(function* () {
          yield* call('chipError', () =>
            loader.writeFlash({
              fileArray: [{ data, address }],
              flashMode: 'keep',
              flashFreq: 'keep',
              flashSize: 'keep',
              eraseAll: false,
              compress: true,
              reportProgress: (_file, written, total) => {
                Queue.offerUnsafe(
                  progress,
                  total === 0
                    ? data.length
                    : Math.floor((data.length * written) / total),
                )
              },
            }),
          )
          const written = yield* digest({ address, size: data.length })
          if (written !== md5Hex(data)) {
            return yield* new FlasherError({
              reason: 'digestMismatch',
              detail: `The chip's MD5 of ${hex(address)}+${hex(data.length)} differs from the data written.`,
            })
          }
          yield* Queue.end(progress)
        }),
      )

    const reboot = watchdogReset.pipe(
      Effect.andThen(Ref.set(standing, 'rebooted')),
    )

    return {
      chip,
      read,
      write,
      digest,
      reboot,
      log: Stream.fromQueue(lines),
      lost: Deferred.await(gone),
    } satisfies Session
  })

const noop = () => {}
