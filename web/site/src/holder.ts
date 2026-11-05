import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import { modifyFields } from 'foldkit/struct'

import {
  type BrowserMsg,
  type Lease,
  MAX_DETAIL_CHARS,
  type Op,
  type OpFailure,
  type OpOutcome,
  type Region,
  type RequestId,
} from '@cycling/protocol'

import { labelOf } from './flasher/partitions.ts'
import { totalOf } from './flasher/run.ts'
import { portChanged, reacquire } from './live.ts'
import type { Finished } from './message.ts'
import type {
  Activity,
  ActivityKind,
  Hold,
  Layout,
  Port,
  PortPhase,
  QueuedOp,
  RunnableOp,
} from './model.ts'
import { ACTIVITY_ROWS, OP_QUEUE } from './policy.ts'

/**
 * Who holds the SerialPort, as one table over `Port`. The console holds it
 * until an Enter; then a hold under the agent's lease (`Entering`, then
 * `Flashing` once the flasher has it). Every way back to the console is
 * `endHold`: the port re-keys to a console requirement, which closes the
 * flasher session, and its finalizer resets the chip unless a Reboot already
 * did. Each op is answered exactly once, by this table, never by the runner.
 *
 *  state         event                          next                               answers
 *  ------------  -----------------------------  ---------------------------------  ------------------------------
 *  any           Op Release                     console Reacquiring if held        Released; head stopped,
 *                                                                                  queued notEntered
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
 *  Entering      FailedEntry lostPort, once     Entering{noReset, retried}         -
 *  Entering      FailedEntry otherwise          console Reacquiring                head entryFailed, rest
 *                                                                                  notEntered
 *  Flashing      FinishedOp Entered|Read|...    ops=tail                           the outcome
 *  Flashing      FinishedOp Rebooting           console Reacquiring; queued        Rebooting; the rest notEntered
 *                                               Enters wait for the console
 *  Flashing      FinishedOp Failed chipError,   console Reacquiring                Failed; rest notEntered
 *                portLost or entryFailed
 *  Flashing      FinishedOp Failed otherwise    ops=tail (flash untouched)         Failed
 *  Flashing      LostPort                       console Reacquiring                head portLost, rest notEntered
 *                (head Reboot: as FinishedOp Rebooting)
 *  Flashing      Restart (visitor), ops empty   console Reacquiring                -
 *  Flashing      Not now (visitor), head Write  ops=tail                           notAllowed
 *  held          agent away AGENT_GRACE_MS      console Reacquiring                head stopped, rest notEntered
 *  Reacquiring   OpenedPort, head Enter         Open, then Entering                -
 *  starting      FailedPort                     Failed                             queued Enters entryFailed
 */

export interface Step {
  readonly port: Port
  /** OpResults and PortChanged, in wire order. */
  readonly messages: ReadonlyArray<BrowserMsg>
  /** An Enter to answer from the held session, so the MAC skips `update`. */
  readonly entered: Option.Option<RequestId>
  /** The agent left the hold; start the wait for it. */
  readonly awaitAgent: Option.Option<{
    readonly lease: Lease
    readonly departures: number
  }>
}

const step = (
  port: Port,
  messages: ReadonlyArray<BrowserMsg> = [],
  extra: Partial<Pick<Step, 'entered' | 'awaitAgent'>> = {},
): Step => ({
  port,
  messages,
  entered: extra.entered ?? Option.none(),
  awaitAgent: extra.awaitAgent ?? Option.none(),
})

const answer = (id: RequestId, outcome: OpOutcome): BrowserMsg => ({
  _tag: 'OpResult',
  id,
  outcome,
})

const refusal = (id: RequestId, reason: OpFailure, detail = ''): BrowserMsg =>
  answer(id, {
    _tag: 'Failed',
    reason,
    detail: detail.slice(0, MAX_DETAIL_CHARS),
  })

export const isHold = (phase: PortPhase): phase is Hold =>
  phase._tag === 'Entering' || phase._tag === 'Flashing'

type Flashing = Extract<PortPhase, { readonly _tag: 'Flashing' }>
type Entering = Extract<PortPhase, { readonly _tag: 'Entering' }>

const isFlashing = (phase: PortPhase): phase is Flashing =>
  phase._tag === 'Flashing'

export const layoutOf = (port: Port): Option.Option<Layout> =>
  isFlashing(port.state) ? port.state.layout : Option.none()

const regionOf = (op: Op): Option.Option<Region> =>
  Match.value(op).pipe(
    Match.tag('Read', 'Write', 'Digest', ({ region }) => Option.some(region)),
    Match.orElse(() => Option.none()),
  )

/** The head while the flasher holds the port: the op that is running, or waiting for the visitor. */
export const headOf = (port: Port): Option.Option<QueuedOp> =>
  isFlashing(port.state) ? Option.fromNullishOr(port.ops[0]) : Option.none()

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
  retried: false,
  departures: 0,
})

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

/** An Op from the agent. Always a step: every op is answered. */
export const onOp = (port: Port, id: RequestId, op: Op): Step =>
  Match.value(op).pipe(
    Match.withReturnType<Step>(),
    Match.tagsExhaustive({
      Release: () => release(port, id),
      Enter: entry => enter(port, id, entry, entry.lease),
      Read: read => holdOp(port, id, read, read.lease),
      Write: write => holdOp(port, id, write, write.lease),
      Digest: digest => holdOp(port, id, digest, digest.lease),
      Reboot: reboot => holdOp(port, id, reboot, reboot.lease),
    }),
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
    departures: phase.departures,
  })
  return Option.some(step(next, [portChanged(next)]))
}

export const onFailedEntry = (
  port: Port,
  failure: { readonly lostPort: boolean; readonly detail: string },
): Option.Option<Step> => {
  const phase = port.state
  if (phase._tag !== 'Entering') {
    return Option.none()
  }
  if (failure.lostPort && !phase.retried) {
    return Option.some(
      step(withState(port, { ...phase, entry: 'noReset', retried: true })),
    )
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
          : [refusal(head.id, 'entryFailed', failure.detail)]),
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
  headOf(port).pipe(
    Option.filter(head => port.attempt === attempt && head.id === id),
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
        Match.tagsExhaustive({
          Entered: () =>
            step(withOps(recordOp(port, head, 'done'), rest), [], {
              entered: Option.some(id),
            }),
          Read: outcome =>
            step(withOps(recordOp(port, head, 'done'), rest), [
              answer(id, outcome),
            ]),
          Written: outcome =>
            step(withOps(recordOp(port, head, 'done'), rest), [
              answer(id, outcome),
            ]),
          Digest: outcome =>
            step(withOps(recordOp(port, head, 'done'), rest), [
              answer(id, outcome),
            ]),
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
              : step(withOps(recordOp(port, head, 'failed'), rest), [
                  answer(id, outcome),
                ]),
        }),
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
  return headOf(port).pipe(
    Option.filter(head => head.op._tag === 'Reboot'),
    Option.flatMap(head =>
      onFinishedOp(port, attempt, head.id, { _tag: 'Rebooting' }),
    ),
    Option.orElse(() => Option.some(abandonHold(port, 'portLost'))),
  )
}

/** The visitor's way back to a normal device, offered while update mode is idle. */
export const onRestartDevice = (port: Port): Option.Option<Step> =>
  isFlashing(port.state) && port.ops.length === 0
    ? Option.some(
        endHold(record(port, 'Restart', Option.none(), 'done'), [], []),
      )
    : Option.none()

/** The head Write waits for the visitor; Not now refuses it and the hold goes on. */
export const onDeclineWrite = (port: Port): Option.Option<Step> =>
  headOf(port).pipe(
    Option.filter(head => head.op._tag === 'Write'),
    Option.map(head =>
      step(withOps(recordOp(port, head, 'declined'), port.ops.slice(1)), [
        refusal(head.id, 'notAllowed'),
      ]),
    ),
  )

export const onAgentLeft = (port: Port): Option.Option<Step> => {
  const phase = port.state
  if (!isHold(phase)) {
    return Option.none()
  }
  const departures = phase.departures + 1
  return Option.some(
    step(withState(port, { ...phase, departures }), [], {
      awaitAgent: Option.some({ lease: phase.lease, departures }),
    }),
  )
}

/** Only the latest absence counts, and only if the agent is still gone. */
export const onAgentAwayElapsed = (
  port: Port,
  away: { readonly lease: Lease; readonly departures: number },
  agentPresent: boolean,
): Option.Option<Step> => {
  const phase = port.state
  return isHold(phase) &&
    !agentPresent &&
    phase.lease === away.lease &&
    phase.departures === away.departures
    ? Option.some(abandonHold(port, 'stopped'))
    : Option.none()
}

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

/** A write is running: what Disconnect asks about. */
export const runningWrite = (
  port: Port,
  writesAllowed: boolean,
): Option.Option<Extract<RunnableOp, { readonly _tag: 'Write' }>> =>
  headOf(port).pipe(
    Option.map(head => head.op),
    Option.filter(
      (op): op is Extract<RunnableOp, { readonly _tag: 'Write' }> =>
        op._tag === 'Write' && writesAllowed,
    ),
  )
