import * as Duration from 'effect/Duration'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Queue from 'effect/Queue'
import * as Ref from 'effect/Ref'
import * as Schema from 'effect/Schema'

import {
  type Assembly,
  MAX_FLASH_BYTES,
  OP_STALL_MS,
  type OpData,
  type RequestId,
  assemble,
  assembled,
  emptyAssembly,
} from '@cycling/protocol'

export class PayloadRefused extends Schema.TaggedError<
  PayloadRefused,
  { readonly brand: unique symbol }
>()('PayloadRefused', {
  reason: Schema.Literals(['transferGap', 'payloadMismatch']),
}) {}

/**
 * Write payloads from the relay, by op id, kept outside `update`. Frames may
 * arrive before the op is queued or long before it reaches the head, so the
 * inbox keeps them until the runner takes them.
 */
export interface Inbox {
  readonly offer: (frame: OpData) => Effect.Effect<void>
  /**
   * The whole payload of op `id`: contiguous from 0, exactly `size` bytes,
   * with MD5 `md5`. A hole, an overrun or `OP_STALL_MS` without a frame is
   * `transferGap`; a different MD5 is `payloadMismatch`.
   */
  readonly take: (
    id: RequestId,
    size: number,
    md5: string,
  ) => Effect.Effect<Uint8Array, PayloadRefused>
}

/**
 * The running op's payload and the one queued behind it. A third id evicts
 * the least recently fed, which is how a refused write's frames go away.
 */
const KEPT_PAYLOADS = 2

interface Slot {
  readonly id: RequestId
  /** None once a frame left a hole; only a frame at offset 0 starts it over. */
  readonly assembly: Option.Option<Assembly>
}

const fed = (slot: Option.Option<Slot>, frame: OpData): Slot => ({
  id: frame.id,
  assembly:
    frame.offset === 0
      ? assemble(emptyAssembly(MAX_FLASH_BYTES), frame)
      : Option.flatMap(
          Option.flatMap(slot, ({ assembly }) => assembly),
          assembly => assemble(assembly, frame),
        ),
})

export const makeInbox = (
  md5Hex: (bytes: Uint8Array) => string,
): Effect.Effect<Inbox> =>
  Effect.gen(function* () {
    const slots = yield* Ref.make<ReadonlyArray<Slot>>([])
    const doorbell = yield* Queue.sliding<true>(1)

    const offer = (frame: OpData) =>
      Ref.update(slots, current =>
        [
          ...current.filter(slot => slot.id !== frame.id),
          fed(
            Option.fromNullishOr(current.find(slot => slot.id === frame.id)),
            frame,
          ),
        ].slice(-KEPT_PAYLOADS),
      ).pipe(Effect.andThen(Queue.offer(doorbell, true)), Effect.asVoid)

    const refused = (reason: PayloadRefused['reason']) =>
      Effect.fail(new PayloadRefused({ reason }))

    const ready = (id: RequestId, size: number) =>
      Ref.get(slots).pipe(
        Effect.flatMap(current =>
          Option.match(
            Option.fromNullishOr(current.find(slot => slot.id === id)),
            {
              onNone: () => Effect.succeed(Option.none<Uint8Array>()),
              onSome: ({ assembly }) =>
                Option.match(assembly, {
                  onNone: () => refused('transferGap'),
                  onSome: progress =>
                    progress.received > size
                      ? refused('transferGap')
                      : Effect.succeed(
                          progress.received === size
                            ? Option.some(assembled(progress))
                            : Option.none(),
                        ),
                }),
            },
          ),
        ),
      )

    const nextFrame = Queue.take(doorbell).pipe(
      Effect.timeoutOrElse({
        duration: Duration.millis(OP_STALL_MS),
        orElse: () => refused('transferGap'),
      }),
    )

    const take = (id: RequestId, size: number, md5: string) =>
      Effect.gen(function* () {
        while (true) {
          const payload = yield* ready(id, size)
          if (Option.isSome(payload)) {
            return payload.value
          }
          yield* nextFrame
        }
      }).pipe(
        Effect.ensuring(
          Ref.update(slots, current => current.filter(slot => slot.id !== id)),
        ),
        Effect.filterOrElse(
          payload => md5Hex(payload) === md5,
          () => refused('payloadMismatch'),
        ),
      )

    return { offer, take }
  })
