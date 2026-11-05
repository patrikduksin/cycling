import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Match from 'effect/Match'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'
import * as Stream from 'effect/Stream'

import { FlasherError, type Session } from '@cycling/flasher'
import {
  type Binary,
  type BrowserMsg,
  MAX_DETAIL_CHARS,
  Md5,
  OP_PROGRESS_MS,
  type Op,
  type Region,
  type RequestId,
  opDataFrames,
} from '@cycling/protocol'

import { type Finished, Message } from '../message.ts'
import type { RunnableOp } from '../model.ts'
import type { Inbox, PayloadRefused } from './inbox.ts'

export type Send<R> = (
  item: Binary | BrowserMsg,
) => Effect.Effect<boolean, never, R>

export interface RunOptions<R> {
  /** The port attempt the session belongs to, so `update` can drop a late result. */
  readonly attempt: number
  readonly id: RequestId
  readonly op: RunnableOp
  readonly session: Session
  readonly inbox: Inbox
  readonly send: Send<R>
}

/** What an op's progress counts: the region's bytes, or nothing for Enter and Reboot. */
export const totalOf = (op: Op): number =>
  Match.value(op).pipe(
    Match.tag('Read', 'Write', 'Digest', ({ region }) => region.size),
    Match.orElse(() => 0),
  )

const clip = (detail: string): string => detail.slice(0, MAX_DETAIL_CHARS)

const decodeMd5 = Schema.decodeUnknownEffect(Md5)

const digestOf = (session: Session, region: Region) =>
  session.digest(region).pipe(
    Effect.flatMap(hex =>
      decodeMd5(hex).pipe(
        Effect.mapError(
          () =>
            new FlasherError({
              reason: 'chipError',
              detail: 'The chip sent a malformed digest.',
            }),
        ),
      ),
    ),
  )

const failed =
  (reason: Extract<Finished, { _tag: 'Failed' }>['reason']) =>
  (detail: string): Finished => ({
    _tag: 'Failed',
    reason,
    detail: clip(detail),
  })

const perform = <R>(
  options: RunOptions<R>,
  progress: (done: number) => Effect.Effect<void>,
): Effect.Effect<Finished, never, R> => {
  const { session, id } = options
  return Match.value(options.op).pipe(
    Match.withReturnType<
      Effect.Effect<Finished, FlasherError | PayloadRefused, R>
    >(),
    Match.tagsExhaustive({
      Enter: () => Effect.succeed({ _tag: 'Entered' }),
      Read: ({ region }) =>
        session.read(region).pipe(
          Stream.runForEach(event =>
            event._tag === 'Progress'
              ? progress(event.done)
              : Effect.forEach(
                  opDataFrames(id, event.bytes, event.offset),
                  frame => options.send(frame),
                  { discard: true },
                ),
          ),
          Effect.andThen(digestOf(session, region)),
          Effect.map((md5): Finished => ({ _tag: 'Read', md5 })),
        ),
      Write: ({ region, md5 }) =>
        options.inbox.take(id, region.size, md5).pipe(
          Effect.flatMap(payload =>
            session
              .write(region.address, payload)
              .pipe(Stream.runForEach(progress)),
          ),
          Effect.andThen(digestOf(session, region)),
          Effect.map((written): Finished => ({
            _tag: 'Written',
            md5: written,
          })),
        ),
      Digest: ({ region }) =>
        digestOf(session, region).pipe(
          Effect.map((md5): Finished => ({ _tag: 'Digest', md5 })),
        ),
      Reboot: () =>
        session.reboot.pipe(Effect.as<Finished>({ _tag: 'Rebooting' })),
    }),
    Effect.catchTags({
      FlasherError: error => Effect.succeed(failed(error.reason)(error.detail)),
      PayloadRefused: ({ reason }) => Effect.succeed(failed(reason)('')),
    }),
  )
}

/**
 * One op against the held session. OpData and OpProgress go to the relay
 * from here; `update` hears `ProgressedOp` once a second and one
 * `FinishedOp`, and answers the op itself. The progress beat runs while the
 * chip reports nothing too (an erase, a digest), so the agent never mistakes
 * a slow op for a stalled one, and it stops before `FinishedOp` is emitted.
 */
export const runOp = <R>(
  options: RunOptions<R>,
): Stream.Stream<Message, never, R> =>
  Stream.callback<Message, never, R>(queue =>
    Effect.gen(function* () {
      const { attempt, id } = options
      const total = totalOf(options.op)
      const done = yield* Ref.make(0)
      const beat = Ref.get(done).pipe(
        Effect.tap(now =>
          options.send({ _tag: 'OpProgress', id, done: now, total }),
        ),
        Effect.flatMap(now =>
          Queue.offer(
            queue,
            Message.ProgressedOp({ attempt, id, done: now, total }),
          ),
        ),
        Effect.andThen(Effect.sleep(Duration.millis(OP_PROGRESS_MS))),
        Effect.forever,
      )
      const finished = yield* Effect.scoped(
        Effect.forkScoped(beat).pipe(
          Effect.andThen(perform(options, now => Ref.set(done, now))),
        ),
      )
      yield* Queue.offer(queue, Message.FinishedOp({ attempt, id, finished }))
      yield* Queue.end(queue)
    }),
  )
