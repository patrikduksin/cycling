import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import { modifyFields } from 'foldkit/struct'

import {
  type BrowserMsg,
  type Lease,
  type Op,
  type OpFailure,
  type OpOutcome,
  type Region,
  type RequestId,
} from '@cycling/protocol'

import { labelOf } from './flasher/partitions.ts'
import { totalOf } from './flasher/run.ts'
import type { EntryFailure } from './links.ts'
import { type Step, portChanged, reacquire, step } from './live.ts'
import type {
  Activity,
  ActivityKind,
  Finished,
  Hold,
  Layout,
  Port,
  PortPhase,
  QueuedOp,
  RunnableOp,
} from './model.ts'
import {
  ACTIVITY_ROWS,
  AGENT_GRACE_MS,
  IDLE_HOLD_MS,
  OP_QUEUE,
} from './policy.ts'

/**
 * Who holds the SerialPort, as one table over `Port`. The console holds it
 * until an Enter; then a hold under the agent's lease (`Entering`, then
 * `Flashing` once the flasher has it). Every way back to the console is
 * `endHold`: the port re-keys to a console requirement, which closes the
 * flasher session, and its finalizer resets the chip unless a Reboot already
 * did. Each op is answered exactly once, by this table, never by the runner;
 * the agent's resend of an op (same id, same lease) joins it instead.
 *
 *  state         event                          next                               answers
 *  ------------  -----------------------------  ---------------------------------  ------------------------------
 *  any           Op Release                     console Reacquiring if held        Released; head stopped,
 *                                                                                  queued notEntered
 *  any           Op X{lease, id} already queued -                                  - (its answer will come)
 *  Flashing      Op X{lease, id} last answered  -                                  that answer again
 *  Open          Op Enter                       Entering{usbReset}, ops=[Enter]    -
 *  Opening,      Op Enter                       ops+=[Enter]; it enters once the   -
 *  Reacquiring                                  console is Open again
 *  Failed        Op Enter                       -                                  entryFailed
 *  console       Op Read|Write|Digest|Reboot    -                                  notEntered
 *  held          Op Enter{any lease}            lease=it, ops+=[Enter]             Entered when it runs; another
 *                                                                                  lease takes the hold over
 *  held          Op X{other lease}              -                                  notEntered
 *  any           an op past OP_QUEUE            -                                  busy
 *  held          Op X                           ops+=[X]                           -
 *  Entering      EnteredFlasher                 Flashing                           PortChanged Flashing
 *  Entering      FailedEntry portLost, usbReset Entering{noReset}                  -
 *  Entering      FailedEntry otherwise          console Reacquiring                head entryFailed (or
 *                                                                                  unsupportedChip), rest notEntered
 *  Flashing      FinishedOp Entered|Read|...    ops=tail, answered=it              the outcome
 *  Flashing      FinishedOp Rebooting           console Reacquiring; queued        Rebooting; the rest notEntered
 *                                               Enters wait for the console
 *  Flashing      FinishedOp Failed chipError,   console Reacquiring                Failed; rest notEntered
 *                portLost or entryFailed
 *  Flashing      FinishedOp Failed otherwise    ops=tail, answered=it              Failed
 *  Flashing      LostPort                       console Reacquiring                head portLost, rest notEntered
 *                (head Reboot: as FinishedOp Rebooting)
 *  Flashing      Restart (visitor), Idle        console Reacquiring                -
 *  Flashing      Not now (visitor), consent     ops=tail, answered=it              notAllowed
 *  Flashing      ExpiredHold, nothing running   console Reacquiring                head stopped, rest notEntered
 *  Reacquiring   OpenedPort, head Enter         Open, then Entering                -
 *  starting      FailedPort                     Failed                             queued Enters entryFailed
 */

const answer = (id: RequestId, outcome: OpOutcome): BrowserMsg => ({
  _tag: 'OpResult',
  id,
  outcome,
})

const refusal = (id: RequestId, reason: OpFailure, detail = ''): BrowserMsg =>
  answer(id, { _tag: 'Failed', reason, detail })

export const isHold = (phase: PortPhase): phase is Hold =>
  phase._tag === 'Entering' || phase._tag === 'Flashing'

type Flashing = Extract<PortPhase, { readonly _tag: 'Flashing' }>
type Entering = Extract<PortPhase, { readonly _tag: 'Entering' }>

const isFlashing = (phase: PortPhase): phase is Flashing =>
  phase._tag === 'Flashing'

export const layoutOf = (port: Port): Option.Option<Layout> =>
  isFlashing(port.state) ? port.state.layout : Option.none()

/**
 * What the hold is doing, the one reading of it for the op runner, the
 * heartbeats, the hold's expiry, the update panel and the visitor's buttons.
 */
export type HeadState =
  | { readonly _tag: 'Console' }
  | { readonly _tag: 'Entering'; readonly head: Option.Option<QueuedOp> }
  /** In update mode with nothing queued: Restart is offered, and the hold expires. */
  | { readonly _tag: 'Idle' }
  | { readonly _tag: 'Running'; readonly head: QueuedOp }
  /** The head Write waits for the visitor's Allow. */
  | {
      readonly _tag: 'AwaitingConsent'
      readonly head: QueuedOp
      readonly region: Region
    }

export const headState = (port: Port, writesAllowed: boolean): HeadState => {
  const head = port.ops[0]
  return Match.value(port.state).pipe(
    Match.withReturnType<HeadState>(),
    Match.tag('Entering', () => ({
      _tag: 'Entering',
      head: Option.fromNullishOr(head),
    })),
    Match.tag('Flashing', () =>
      head === undefined
        ? { _tag: 'Idle' }
        : head.op._tag === 'Write' && !writesAllowed
          ? { _tag: 'AwaitingConsent', head, region: head.op.region }
          : { _tag: 'Running', head },
    ),
    Match.orElse(() => ({ _tag: 'Console' })),
  )
}

/** A write is running: what Disconnect asks about. */
export const runningWrite = (
  port: Port,
  writesAllowed: boolean,
): Option.Option<Extract<RunnableOp, { readonly _tag: 'Write' }>> => {
  const state = headState(port, writesAllowed)
  return state._tag === 'Running' && state.head.op._tag === 'Write'
    ? Option.some(state.head.op)
    : Option.none()
}

/** Every queued op the runner is not running: each still tells the agent it is alive. */
export const waitingOps = (
  port: Port,
  writesAllowed: boolean,
): ReadonlyArray<QueuedOp> => {
  const state = headState(port, writesAllowed)
  return state._tag === 'Running' ? port.ops.slice(1) : port.ops
}

/**
 * When an unattended hold gives the device back: never while an op runs or
 * the flasher is entering; `AGENT_GRACE_MS` after the agent left (or the
 * page lost the relay); `IDLE_HOLD_MS` of nothing while the agent is here,
 * which is how a Release lost on the way still ends the hold.
 */
export const holdExpiry = (
  port: Port,
  writesAllowed: boolean,
  agentHere: boolean,
): Option.Option<{
  readonly attempt: number
  readonly lease: Lease
  readonly after: number
}> => {
  const phase = port.state
  const state = headState(port, writesAllowed)
  if (!isFlashing(phase)) {
    return Option.none()
  }
  const after = Match.value(state).pipe(
    Match.withReturnType<Option.Option<number>>(),
    Match.tag('Idle', () =>
      Option.some(agentHere ? IDLE_HOLD_MS : AGENT_GRACE_MS),
    ),
    Match.tag('AwaitingConsent', () =>
      agentHere ? Option.none() : Option.some(AGENT_GRACE_MS),
    ),
    Match.orElse(() => Option.none()),
  )
  return Option.map(after, ms => ({
    attempt: port.attempt,
    lease: phase.lease,
    after: ms,
  }))
}

const regionOf = (op: Op): Option.Option<Region> =>
  Match.value(op).pipe(
    Match.tag('Read', 'Write', 'Digest', ({ region }) => Option.some(region)),
    Match.orElse(() => Option.none()),
  )

const record = (
  port: Port,
  kind: ActivityKind,
  region: Option.Option<Region>,
  result: Activity['result'],
): Port =>
  modifyFields(port, {
    activity: activity =>
      [
        {
          seq: (activity[0]?.seq ?? 0) + 1,
          kind,
          region,
          label: Option.flatMap(region, at => labelOf(layoutOf(port), at)),
          result,
        },
        ...activity,
      ].slice(0, ACTIVITY_ROWS),
  })

const recordOp = (
  port: Port,
  queued: QueuedOp,
  result: Activity['result'],
): Port => record(port, queued.op._tag, regionOf(queued.op), result)

const withOps = (port: Port, ops: ReadonlyArray<QueuedOp>): Port =>
  modifyFields(port, { ops: () => ops })

const withState = (port: Port, state: PortPhase): Port =>
  modifyFields(port, { state: () => state })

const queued = (id: RequestId, op: RunnableOp): QueuedOp => ({
  id,
  op,
  done: 0,
  total: totalOf(op),
})

const enqueue = (port: Port, id: RequestId, op: RunnableOp): Port =>
  withOps(port, [...port.ops, queued(id, op)])

const entering = (lease: Lease): Entering => ({
  _tag: 'Entering',
  lease,
  entry: 'usbReset',
})

/** The wire's answer to a finished op; None for Entered, which the session answers. */
const outcomeOf = (finished: Finished): Option.Option<OpOutcome> =>
  finished._tag === 'Entered' ? Option.none() : Option.some(finished)

const answerStep = (port: Port, id: RequestId, finished: Finished): Step =>
  Option.match(outcomeOf(finished), {
    onNone: () => step(port, [], Option.some(id)),
    onSome: outcome => step(port, [answer(id, outcome)]),
  })

/** The head is done and stays answerable: it leaves the queue and becomes the hold's last answer. */
const settleHead = (
  port: Port,
  head: QueuedOp,
  result: Activity['result'],
  finished: Finished,
): Step => {
  const phase = port.state
  const settled = withOps(recordOp(port, head, result), port.ops.slice(1))
  return answerStep(
    isFlashing(phase)
      ? withState(settled, {
          ...phase,
          answered: Option.some({ id: head.id, finished }),
        })
      : settled,
    head.id,
    finished,
  )
}

/**
 * Back to the console. Bumping `attempt` and leaving the hold re-keys the
 * port: the flasher session closes, then the console waits for the device,
 * and the wire walks Lost, Reacquiring and later Open with a new epoch.
 */
const endHold = (
  port: Port,
  ops: ReadonlyArray<QueuedOp>,
  answers: ReadonlyArray<BrowserMsg>,
): Step => {
  const back = reacquire(port)
  return step(withOps(back.port, ops), [...answers, ...back.messages])
}

/** Ends the hold for a reason outside the ops: the head stops, the rest were never entered. */
const abandonHold = (
  port: Port,
  headReason: OpFailure,
  answers: ReadonlyArray<BrowserMsg> = [],
): Step => {
  const [head, ...rest] = port.ops
  const settled = head === undefined ? port : recordOp(port, head, 'stopped')
  return endHold(
    settled,
    [],
    [
      ...answers,
      ...(head === undefined ? [] : [refusal(head.id, headReason)]),
      ...rest.map(op => refusal(op.id, 'notEntered')),
    ],
  )
}

const release = (port: Port, id: RequestId): Step => {
  const released = answer(id, { _tag: 'Released' })
  return isHold(port.state)
    ? abandonHold(port, 'stopped', [released])
    : step(withOps(port, []), [
        released,
        ...port.ops.map(op => refusal(op.id, 'stopped')),
      ])
}

const enter = (
  port: Port,
  id: RequestId,
  op: RunnableOp,
  lease: Lease,
): Step => {
  const phase = port.state
  if (phase._tag === 'Failed') {
    return step(port, [refusal(id, 'entryFailed')])
  }
  if (port.ops.length >= OP_QUEUE) {
    return step(port, [refusal(id, 'busy')])
  }
  const next = enqueue(port, id, op)
  if (isHold(phase)) {
    return step(withState(next, { ...phase, lease }))
  }
  return phase._tag === 'Open'
    ? step(withState(next, entering(lease)))
    : step(next)
}

const holdOp = (
  port: Port,
  id: RequestId,
  op: RunnableOp,
  lease: Lease,
): Step => {
  const phase = port.state
  if (!isHold(phase) || phase.lease !== lease) {
    return step(port, [refusal(id, 'notEntered')])
  }
  return port.ops.length >= OP_QUEUE
    ? step(port, [refusal(id, 'busy')])
    : step(enqueue(port, id, op))
}

/**
 * The agent resends an op it heard no answer to under the same id: a relay
 * drop, or `OP_STALL_MS` of silence. One still queued or running is joined, so a
 * running write is never run twice or queued behind itself; the last one
 * answered is answered again.
 */
const resent = (
  port: Port,
  id: RequestId,
  lease: Lease,
): Option.Option<Step> => {
  if (port.ops.some(op => op.id === id && op.op.lease === lease)) {
    return Option.some(step(port))
  }
  const phase = port.state
  return isFlashing(phase) && phase.lease === lease
    ? phase.answered.pipe(
        Option.filter(answered => answered.id === id),
        Option.map(answered => answerStep(port, id, answered.finished)),
      )
    : Option.none()
}

/** An Op from the agent. Always a step: every op is answered once. */
export const onOp = (port: Port, id: RequestId, op: Op): Step =>
  op._tag === 'Release'
    ? release(port, id)
    : Option.getOrElse(resent(port, id, op.lease), () =>
        op._tag === 'Enter'
          ? enter(port, id, op, op.lease)
          : holdOp(port, id, op, op.lease),
      )

export const onEnteredFlasher = (
  port: Port,
  layout: Option.Option<Layout>,
): Option.Option<Step> => {
  const phase = port.state
  if (phase._tag !== 'Entering') {
    return Option.none()
  }
  const next = withState(port, {
    _tag: 'Flashing',
    lease: phase.lease,
    entry: phase.entry,
    layout,
    answered: Option.none(),
  })
  return Option.some(step(next, [portChanged(next)]))
}

export const onFailedEntry = (
  port: Port,
  failure: { readonly reason: EntryFailure; readonly detail: string },
): Option.Option<Step> => {
  const phase = port.state
  if (phase._tag !== 'Entering') {
    return Option.none()
  }
  if (failure.reason === 'portLost' && phase.entry === 'usbReset') {
    return Option.some(step(withState(port, { ...phase, entry: 'noReset' })))
  }
  const [head, ...rest] = port.ops
  const settled = head === undefined ? port : recordOp(port, head, 'failed')
  return Option.some(
    endHold(
      settled,
      [],
      [
        ...(head === undefined
          ? []
          : [
              refusal(
                head.id,
                failure.reason === 'unsupportedChip'
                  ? 'unsupportedChip'
                  : 'entryFailed',
                failure.detail,
              ),
            ]),
        ...rest.map(op => refusal(op.id, 'notEntered')),
      ],
    ),
  )
}

/** The runner reports for the head of this hold only; anything else is late. */
const currentHead = (
  port: Port,
  attempt: number,
  id: RequestId,
): Option.Option<QueuedOp> =>
  Option.fromNullishOr(port.ops[0]).pipe(
    Option.filter(
      head =>
        isFlashing(port.state) && port.attempt === attempt && head.id === id,
    ),
  )

export const onProgressedOp = (
  port: Port,
  progress: {
    readonly attempt: number
    readonly id: RequestId
    readonly done: number
    readonly total: number
  },
): Option.Option<Step> =>
  currentHead(port, progress.attempt, progress.id).pipe(
    Option.map(head =>
      step(
        withOps(port, [
          { ...head, done: progress.done, total: progress.total },
          ...port.ops.slice(1),
        ]),
      ),
    ),
  )

/** Failures after which the session itself is not to be trusted. */
const ENDS_HOLD: ReadonlySet<OpFailure> = new Set([
  'chipError',
  'portLost',
  'entryFailed',
])

export const onFinishedOp = (
  port: Port,
  attempt: number,
  id: RequestId,
  finished: Finished,
): Option.Option<Step> =>
  currentHead(port, attempt, id).pipe(
    Option.map(head => {
      const rest = port.ops.slice(1)
      return Match.value(finished).pipe(
        Match.withReturnType<Step>(),
        Match.tags({
          Rebooting: outcome =>
            endHold(
              recordOp(port, head, 'done'),
              rest.filter(op => op.op._tag === 'Enter'),
              [
                answer(id, outcome),
                ...rest
                  .filter(op => op.op._tag !== 'Enter')
                  .map(op => refusal(op.id, 'notEntered')),
              ],
            ),
          Failed: outcome =>
            ENDS_HOLD.has(outcome.reason)
              ? endHold(
                  recordOp(port, head, 'failed'),
                  [],
                  [
                    answer(id, outcome),
                    ...rest.map(op => refusal(op.id, 'notEntered')),
                  ],
                )
              : settleHead(port, head, 'failed', outcome),
        }),
        Match.orElse(done => settleHead(port, head, 'done', done)),
      )
    }),
  )

/**
 * The device vanished while the flasher held it. During a Reboot that is the
 * watchdog reset taking effect, which can beat the op's own result.
 */
export const onLostFlasher = (
  port: Port,
  attempt: number,
): Option.Option<Step> => {
  if (!isFlashing(port.state) || port.attempt !== attempt) {
    return Option.none()
  }
  return Option.fromNullishOr(port.ops[0]).pipe(
    Option.filter(head => head.op._tag === 'Reboot'),
    Option.flatMap(head =>
      onFinishedOp(port, attempt, head.id, { _tag: 'Rebooting' }),
    ),
    Option.orElse(() => Option.some(abandonHold(port, 'portLost'))),
  )
}

/** The visitor's way back to a normal device, offered while update mode is idle. */
export const onRestartDevice = (
  port: Port,
  writesAllowed: boolean,
): Option.Option<Step> =>
  headState(port, writesAllowed)._tag === 'Idle'
    ? Option.some(
        endHold(record(port, 'Restart', Option.none(), 'done'), [], []),
      )
    : Option.none()

/** The head Write waits for the visitor; Not now refuses it and the hold goes on. */
export const onDeclineWrite = (
  port: Port,
  writesAllowed: boolean,
): Option.Option<Step> => {
  const state = headState(port, writesAllowed)
  return state._tag === 'AwaitingConsent'
    ? Option.some(
        settleHead(port, state.head, 'declined', {
          _tag: 'Failed',
          reason: 'notAllowed',
          detail: '',
        }),
      )
    : Option.none()
}

/** `holdExpiry` ran out; a hold whose expiry changed since (an op ran, the agent came back) is kept. */
export const onExpiredHold = (
  port: Port,
  expired: {
    readonly attempt: number
    readonly lease: Lease
    readonly after: number
  },
  writesAllowed: boolean,
  agentHere: boolean,
): Option.Option<Step> =>
  holdExpiry(port, writesAllowed, agentHere).pipe(
    Option.filter(
      due =>
        due.attempt === expired.attempt &&
        due.lease === expired.lease &&
        due.after === expired.after,
    ),
    Option.map(() => abandonHold(port, 'stopped')),
  )

/** The console is back; an Enter that waited behind a Reboot (or arrived meanwhile) starts now. */
export const startQueuedEntry = (port: Port): Port => {
  const head = port.ops[0]
  return port.state._tag === 'Open' &&
    head !== undefined &&
    head.op._tag === 'Enter'
    ? withState(port, entering(head.op.lease))
    : port
}

/** The console never came back: Enters that waited for it cannot enter. */
export const failQueued = (port: Port): Step =>
  step(
    withOps(port, []),
    port.ops.map(op => refusal(op.id, 'entryFailed')),
  )
