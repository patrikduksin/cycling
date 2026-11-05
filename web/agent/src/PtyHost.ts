import * as Context from 'effect/Context'
import * as Effect from 'effect/Effect'
import * as FileSystem from 'effect/FileSystem'
import * as Layer from 'effect/Layer'
import * as PlatformError from 'effect/PlatformError'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import type * as Scope from 'effect/Scope'
import * as Stream from 'effect/Stream'
import { readdirSync, readlinkSync, writeSync } from 'node:fs'

import { LocalResourceError } from './errors.ts'

/** The door every `os.open` devtools tool uses: terminal, logs, export, volumes. */
export interface Pty {
  /** A stable symlink to `/dev/pts/N`; this is `CYCLING_PORT`. */
  readonly path: string
  readonly input: Stream.Stream<Uint8Array>
  /** Never blocks. Returns false when the backlog is full and the bytes were dropped. */
  readonly write: (bytes: Uint8Array) => Effect.Effect<boolean>
  /** Device bytes dropped so far because nothing read the pty. */
  readonly dropped: Effect.Effect<number>
}

/**
 * How a pty comes to exist. Narrow on purpose: a socat child or a Python
 * `pty.openpty()` helper can implement it without touching the bridge.
 */
export class PtyHost extends Context.Service<
  PtyHost,
  {
    readonly open: (
      link: string,
    ) => Effect.Effect<Pty, LocalResourceError, Scope.Scope>
  }
>()('agent/PtyHost') {}

/**
 * Chunks toward the pty that may wait for a reader, on top of the ~15 KiB the
 * kernel holds. Device frames are usually 4 KiB (at most 16 KiB).
 */
export const PTY_BACKLOG_CHUNKS = 32
const STALL_POLL = '20 millis'

const openFds = (): ReadonlySet<string> => new Set(readdirSync('/proc/self/fd'))

const failure = (detail: unknown) =>
  new LocalResourceError({ resource: 'pty', detail: String(detail) })

/** The fd readdir itself used is gone by now, so a failed readlink is not one of ours. */
const linkOf = (fd: string) =>
  Effect.try(() => readlinkSync(`/proc/self/fd/${fd}`)).pipe(
    Effect.option,
    Effect.map(target => ({ fd: Number(fd), target })),
  )

const makeTerminal = (data: NonNullable<Bun.TerminalOptions['data']>) =>
  Effect.gen(function* () {
    const { terminal, created } = yield* Effect.try({
      try: () => {
        const before = openFds()
        const terminal = new Bun.Terminal({ data })
        const created = [...openFds()].filter(fd => !before.has(fd))
        return { terminal, created }
      },
      catch: failure,
    })
    const links = yield* Effect.forEach(created, linkOf)
    const targets = links.flatMap(({ fd, target }) =>
      target._tag === 'Some' ? [{ fd, path: target.value }] : [],
    )
    const slaves = targets.filter(({ path }) => /^\/dev\/pts\/\d+$/.test(path))
    const master = targets.find(({ path }) => path === '/dev/ptmx')
    const slave = slaves[0]
    if (slave === undefined || slaves.length !== 1 || master === undefined) {
      terminal.close()
      return yield* failure(
        `expected one new /dev/pts and a /dev/ptmx fd, found ${targets.map(({ path }) => path).join(', ')}`,
      )
    }
    terminal.setRawMode(true)
    return { terminal, slave: slave.path, master: master.fd }
  })

const isEagain = (error: unknown) =>
  typeof error === 'object' &&
  error !== null &&
  'code' in error &&
  error.code === 'EAGAIN'

/** Bun opens the master O_NONBLOCK, so a full pty is EAGAIN (`WouldBlock`): nothing written yet. */
const writeSome = (fd: number, bytes: Uint8Array) =>
  Effect.try({
    try: () => writeSync(fd, bytes),
    catch: cause =>
      PlatformError.systemError({
        _tag: isEagain(cause) ? 'WouldBlock' : 'Unknown',
        module: 'PtyHost',
        method: 'write',
        pathOrDescriptor: fd,
        cause,
      }),
  }).pipe(
    Effect.catchIf(
      error => error.reason._tag === 'WouldBlock',
      () => Effect.succeed(0),
    ),
    Effect.mapError(failure),
  )

/**
 * `Bun.Terminal` in-process: this process holds the slave open, so a tool
 * closing the pty never hangs up the master. The door survives every tool
 * run and every device re-enumeration, the way tools expect a ttyACM to come
 * back. Bun exposes neither the slave path nor backpressure (`write` buffers
 * without bound and `drain` only follows a buffered write), so the slave path
 * is read from the fds the constructor opened and device bytes go to the
 * master fd with non-blocking writes behind a dropping queue.
 */
export const BunTerminalPtyHost: Layer.Layer<
  PtyHost,
  never,
  FileSystem.FileSystem
> = Layer.effect(
  PtyHost,
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem
    const unlink = (path: string) => fs.remove(path, { force: true })
    return PtyHost.of({
      open: Effect.fn('PtyHost.open')(function* (link: string) {
        const input = yield* Queue.unbounded<Uint8Array>()
        const { slave, master } = yield* Effect.acquireRelease(
          makeTerminal((_terminal, bytes) => {
            Queue.offerUnsafe(input, bytes.slice())
          }),
          ({ terminal }) => Effect.sync(() => terminal.close()),
        )
        yield* Effect.acquireRelease(
          unlink(link).pipe(
            Effect.andThen(fs.symlink(slave, link)),
            Effect.mapError(failure),
          ),
          () => Effect.ignore(unlink(link)),
        )
        const backlog = yield* Queue.dropping<Uint8Array>(PTY_BACKLOG_CHUNKS)
        const dropped = yield* Ref.make(0)
        const deliver = (
          chunk: Uint8Array,
        ): Effect.Effect<void, LocalResourceError> =>
          writeSome(master, chunk).pipe(
            Effect.flatMap(written =>
              written === chunk.length
                ? Effect.void
                : Effect.sleep(STALL_POLL).pipe(
                    Effect.andThen(deliver(chunk.subarray(written))),
                  ),
            ),
          )
        yield* Effect.forkScoped(
          Effect.forever(
            Queue.take(backlog).pipe(Effect.flatMap(deliver)),
          ).pipe(
            Effect.tapError(error =>
              Effect.logError('pty writer stopped', error),
            ),
          ),
        )
        const write = (bytes: Uint8Array) =>
          Queue.offer(backlog, bytes).pipe(
            Effect.tap(accepted =>
              accepted
                ? Effect.void
                : Ref.update(dropped, n => n + bytes.length),
            ),
          )
        return {
          path: link,
          input: Stream.fromQueue(input),
          write,
          dropped: Ref.get(dropped),
        } satisfies Pty
      }),
    })
  }),
)
