import * as Effect from 'effect/Effect'
import * as Queue from 'effect/Queue'
import * as Stream from 'effect/Stream'

const SCRIPT = `${import.meta.dir}/../pty_client.py`

export const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString('hex')
export const unhex = (text: string) => Uint8Array.from(Buffer.from(text, 'hex'))

/** A Python process opening the pty the way devtools does, one command per call. */
export const ptyClient = (path: string) =>
  Effect.gen(function* () {
    const proc = yield* Effect.acquireRelease(
      Effect.sync(() =>
        Bun.spawn(['python3', SCRIPT, path], {
          stdin: 'pipe',
          stdout: 'pipe',
          stderr: 'inherit',
        }),
      ),
      proc => Effect.sync(() => proc.kill()),
    )
    const lines = yield* Queue.unbounded<string>()
    yield* Effect.forkScoped(
      Stream.fromReadableStream({
        evaluate: () => proc.stdout,
        onError: String,
      }).pipe(
        Stream.decodeText,
        Stream.splitLines,
        Stream.runForEach(line => Queue.offer(lines, line)),
      ),
    )
    const send = (command: string) =>
      Effect.sync(() => {
        void proc.stdin.write(`${command}\n`)
        void proc.stdin.flush()
      }).pipe(Effect.andThen(Queue.take(lines)))
    return {
      open: send('open'),
      close: send('close'),
      write: (bytes: Uint8Array) => send(`write ${hex(bytes)}`),
      read: (count: number, seconds: number) =>
        send(`read ${count} ${seconds}`).pipe(
          Effect.map(line => unhex(line.replace(/^got ?/, ''))),
        ),
      drain: (seconds: number) =>
        send(`drain ${seconds}`).pipe(
          Effect.map(line => Number(line.replace('drained ', ''))),
        ),
    }
  })
