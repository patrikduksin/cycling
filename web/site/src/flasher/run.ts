import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import * as Stream from 'effect/Stream'

import { FlasherError } from '@cycling/flasher'
import {
  type Binary,
  type BrowserMsg,
  type Md5,
  OP_PROGRESS_MS,
  type Op,
  type RequestId,
  opDataFrames,
} from '@cycling/protocol'

import type { PageSession } from '../adapters/flasher.ts'
import { Message } from '../message.ts'
import type { Finished, RunnableOp } from '../model.ts'
import type { Inbox, PayloadRefused } from './inbox.ts'

export type Send<R> = (
  item: Binary | BrowserMsg,
) => Effect.Effect<boolean, never, R>

export interface RunOptions<R> {
  /** The port attempt the session belongs to, so `update` can drop a late result. */
  readonly attempt: number
  readonly id: RequestId
  readonly op: RunnableOp
  readonly session: PageSession
  readonly inbox: Inbox
  readonly send: Send<R>
}

/** What an op's progress counts: the region's bytes, or nothing for Enter and Reboot. */
export const totalOf = (op: Op): number =>
  Match.value(op).pipe(
    Match.tag('Read', 'Write', 'Digest', ({ region }) => region.size),
    Match.orElse(() => 0),
  )

const failed = (
  reason: Extract<Finished, { _tag: 'Failed' }>['reason'],
  detail = '',
): Finished => ({ _tag: 'Failed', reason, detail })

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
          Stream.runFoldEffect(
            () => Option.none<Md5>(),
            (md5, event) =>
              Match.value(event).pipe(
                Match.tagsExhaustive({
                  Progress: ({ done }) => progress(done).pipe(Effect.as(md5)),
                  Piece: ({ offset, bytes }) =>
                    Effect.forEach(
                      opDataFrames(id, bytes, offset),
                      frame => options.send(frame),
                      { discard: true },
                    ).pipe(Effect.as(md5)),
                  Read: read => Effect.succeedSome(read.md5),
                }),
              ),
          ),
          Effect.flatMap(
            Option.match({
              onNone: () =>
                Effect.fail(
                  new FlasherError({
                    reason: 'chipError',
                    detail: 'The read ended without its digest.',
                  }),
                ),
              onSome: md5 => Effect.succeed<Finished>({ _tag: 'Read', md5 }),
            }),
          ),
        ),
      Write: ({ region, md5 }) =>
        options.inbox.take(id, region.size, md5).pipe(
          Effect.flatMap(payload =>
            session
              .write(region.address, payload)
              .pipe(Stream.runForEach(progress)),
          ),
          Effect.as<Finished>({ _tag: 'Written', md5 }),
        ),
      Digest: ({ region }) =>
        session
          .digest(region)
          .pipe(Effect.map((md5): Finished => ({ _tag: 'Digest', md5 }))),
      Reboot: () =>
        session.reboot.pipe(Effect.as<Finished>({ _tag: 'Rebooting' })),
    }),
    Effect.catchTags({
      FlasherError: error => Effect.succeed(failed(error.reason, error.detail)),
      PayloadRefused: ({ reason }) => Effect.succeed(failed(reason)),
    }),
  )
}

/**
 * One op against the held session. OpData and OpProgress go to the relay
 * from here; `update` hears `ProgressedOp` every `OP_PROGRESS_MS` and one
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
