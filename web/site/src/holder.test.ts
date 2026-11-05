import { afterAll, describe, expect, test } from 'bun:test'
import * as Option from 'effect/Option'
import type { AnyCommand } from 'foldkit/story'
import { Command, given, message, model, story } from 'foldkit/story'

import {
  type BrowserMsg,
  Lease,
  Md5,
  type OpFailure,
  type OpOutcome,
  type PortState,
} from '@cycling/protocol'

import {
  ClearSession,
  SaveSession,
  SayBye,
  SendControl,
  SendEntered,
} from './command.ts'
import { DEMO_PARTITIONS } from './dev/image.ts'
import { totalOf } from './flasher/run.ts'
import { Message } from './message.ts'
import {
  type Finished,
  type Live,
  type Model,
  type Port,
  type PortPhase,
  type QueuedOp,
  Page,
  Relay,
  type RunnableOp,
  Source,
} from './model.ts'
import { AGENT_GRACE_MS, IDLE_HOLD_MS } from './policy.ts'
import {
  holdExpiryOf,
  portRequirement,
  runningOp,
  waitingOps,
} from './resources.ts'
import { epoch, requestId, serialLive } from './test/fixtures.ts'
import { update } from './update.ts'

/** The fake chip's MAC; no Model these tests reach may contain it. */
const FAKE_MAC = '020000000001'

const reached: Array<Model> = []

/** `update`, remembering every Model it returns. */
const step = (current: Model, next: Message) => {
  const result = update(current, next)
  reached.push(result.model)
  return result
}

afterAll(() => {
  expect(reached.length).toBeGreaterThan(30)
  for (const seen of reached) {
    const text = JSON.stringify(seen)
    expect(text).not.toContain(FAKE_MAC)
    expect(text).not.toContain('02:00:00:00:00:01')
  }
})

const lease = Lease.make('0123456789abcdef')
const otherLease = Lease.make('fedcba9876543210')
const md5 = Md5.make('0123456789abcdef0123456789abcdef')

const enter: RunnableOp = { _tag: 'Enter', lease }
const read: RunnableOp = {
  _tag: 'Read',
  lease,
  region: { address: 0x10000, size: 0x200000 },
}
const write: RunnableOp = {
  _tag: 'Write',
  lease,
  region: { address: 0x760000, size: 0x1e0000 },
  md5,
}
const digest: RunnableOp = {
  _tag: 'Digest',
  lease,
  region: { address: 0x760000, size: 0x1e0000 },
}
const reboot: RunnableOp = { _tag: 'Reboot', lease }

const opMessage = (
  id: number,
  op: RunnableOp | { _tag: 'Release'; lease: Lease },
) =>
  Message.ReceivedFromRelay({
    message: { _tag: 'Op', id: requestId(id), op },
  })

const queued = (id: number, op: RunnableOp): QueuedOp => ({
  id: requestId(id),
  op,
  done: 0,
  total: totalOf(op),
})

const result = (id: number, outcome: OpOutcome): BrowserMsg => ({
  _tag: 'OpResult',
  id: requestId(id),
  outcome,
})

const refused = (id: number, reason: OpFailure, detail = ''): BrowserMsg =>
  result(id, { _tag: 'Failed', reason, detail })

const changed = (n: number, state: PortState): BrowserMsg => ({
  _tag: 'PortChanged',
  status: { epoch: epoch(n), state },
})

/** What every end of a hold puts on the wire: the console walks Lost and Reacquiring. */
const backToConsole = [
  changed(1, { _tag: 'Lost' }),
  changed(1, { _tag: 'Reacquiring' }),
]

const layout = {
  bootloader: 0,
  partitions: DEMO_PARTITIONS,
  bootSlot: Option.some('ota_0'),
}

const flashing = (patch: { readonly lease?: Lease } = {}): PortPhase => ({
  _tag: 'Flashing',
  lease: patch.lease ?? lease,
  entry: 'reset',
  layout,
  answered: Option.none(),
})

const held = (
  patch: {
    readonly ops?: ReadonlyArray<QueuedOp>
    readonly writesAllowed?: boolean
    readonly agentPresent?: boolean
    readonly state?: PortPhase
  } = {},
): Model =>
  serialLive({
    state: patch.state ?? flashing(),
    ops: patch.ops ?? [],
    writesAllowed: patch.writesAllowed ?? false,
    relay: Relay.Joined({
      attempt: 0,
      agentPresent: patch.agentPresent ?? true,
    }),
  })

const liveOf = (next: Model): Live => {
  if (!Page.guards.Live(next.page)) {
    throw new Error(`expected Live, got ${next.page._tag}`)
  }
  return next.page
}

const portOf = (next: Model): Port => {
  const { source } = liveOf(next)
  if (!Source.guards.Serial(source)) {
    throw new Error('expected a serial source')
  }
  return source.port
}

const commandsOf = (outcome: {
  readonly commands?: ReadonlyArray<AnyCommand>
}) => (outcome.commands ?? []).map(({ name, args }) => ({ name, args }))

const sent = (...messages: ReadonlyArray<BrowserMsg>) => ({
  name: 'SendControl',
  args: { messages },
})

const finish = (id: number, finished: Finished, attempt = 0) =>
  Message.FinishedOp({ attempt, id: requestId(id), finished })

describe('entering update mode', () => {
  test('Enter from the console re-keys the port to the flasher and says Flashing once it holds it', () => {
    story(
      step,
      given(serialLive()),
      message(opMessage(1, enter)),
      Command.expectNone(),
      model(next => {
        expect(portOf(next).state).toMatchObject({
          _tag: 'Entering',
          lease,
          entry: 'reset',
        })
        expect(portRequirement(next)).toMatchObject(
          Option.some({ _tag: 'Flasher', attempt: 0, entry: 'reset' }),
        )
        expect(runningOp(next)).toEqual(Option.none())
        expect(waitingOps(next)).toEqual([{ id: requestId(1), total: 0 }])
      }),
      message(Message.EnteredFlasher({ layout })),
      Command.resolve(
        SendControl({ messages: [changed(1, { _tag: 'Flashing' })] }),
        Message.CompletedSendControl(),
      ),
      model(next => {
        expect(portOf(next).state._tag).toBe('Flashing')
        expect(runningOp(next)).toEqual(
          Option.some({ attempt: 0, id: requestId(1), op: enter }),
        )
      }),
      message(finish(1, { _tag: 'Entered' })),
      Command.resolve(
        SendEntered({ id: requestId(1) }),
        Message.CompletedSendControl(),
      ),
      model(next => {
        expect(portOf(next).ops).toEqual([])
        expect(portOf(next).activity).toMatchObject([
          { kind: 'Enter', result: 'done' },
        ])
      }),
    )
  })

  test('an Enter with the same lease while held queues an answer and does not re-key', () => {
    const before = held()
    const again = step(before, opMessage(2, enter))
    expect(commandsOf(again)).toEqual([])
    expect(portRequirement(again.model)).toEqual(portRequirement(before))
    expect(portOf(again.model).ops).toEqual([queued(2, enter)])
    expect(
      commandsOf(step(again.model, finish(2, { _tag: 'Entered' }))),
    ).toEqual([{ name: SendEntered.name, args: { id: requestId(2) } }])
  })

  test('an Enter with another lease takes the hold over without a reset', () => {
    const before = held()
    const takeover = step(
      before,
      opMessage(3, { _tag: 'Enter', lease: otherLease }),
    )
    expect(portOf(takeover.model).state).toMatchObject({ lease: otherLease })
    expect(portRequirement(takeover.model)).toEqual(portRequirement(before))
    const stale = step(takeover.model, opMessage(4, read))
    expect(commandsOf(stale)).toEqual([sent(refused(4, 'notEntered'))])
  })

  test('an Enter while the console is coming back waits for it, then enters', () => {
    const waiting = step(
      serialLive({ attempt: 1, state: { _tag: 'Reacquiring' } }),
      opMessage(1, enter),
    )
    expect(portOf(waiting.model).state._tag).toBe('Reacquiring')
    const opened = step(waiting.model, Message.OpenedPort())
    expect(portOf(opened.model).state).toMatchObject({
      _tag: 'Entering',
      lease,
    })
    expect(commandsOf(opened)[0]).toEqual(
      sent(changed(2, { _tag: 'Open', baud: 115_200 })),
    )
  })

  test('an Enter on a failed port is entryFailed', () => {
    const failed = serialLive({
      state: { _tag: 'Failed', reason: 'reacquireTimeout' },
    })
    expect(commandsOf(step(failed, opMessage(1, enter)))).toEqual([
      sent(refused(1, 'entryFailed')),
    ])
  })

  test('an entry that lost the port retries once without a reset, then fails entryFailed', () => {
    const entering = step(serialLive(), opMessage(1, enter)).model
    const retry = step(
      entering,
      Message.FailedEntry({ reason: 'portLost', detail: 'gone' }),
    )
    expect(commandsOf(retry)).toEqual([])
    expect(portRequirement(retry.model)).toMatchObject(
      Option.some({ _tag: 'Flasher', entry: 'noReset' }),
    )
    const gave = step(
      retry.model,
      Message.FailedEntry({ reason: 'portLost', detail: 'gone again' }),
    )
    expect(commandsOf(gave)).toEqual([
      sent(refused(1, 'entryFailed', 'gone again'), ...backToConsole),
    ])
    expect(portOf(gave.model)).toMatchObject({
      attempt: 1,
      state: { _tag: 'Reacquiring' },
      ops: [],
    })
    expect(portRequirement(gave.model)).toMatchObject(
      Option.some({ _tag: 'Console', attempt: 1 }),
    )
  })

  test('an entry that fails another way is entryFailed at once', () => {
    const entering = step(serialLive(), opMessage(1, enter)).model
    const failed = step(
      entering,
      Message.FailedEntry({ reason: 'entryFailed', detail: 'no sync' }),
    )
    expect(commandsOf(failed)).toEqual([
      sent(refused(1, 'entryFailed', 'no sync'), ...backToConsole),
    ])
  })

  test('a chip that is not an ESP32-S3 is unsupportedChip, not retried', () => {
    const entering = step(serialLive(), opMessage(1, enter)).model
    const refusedChip = step(
      entering,
      Message.FailedEntry({
        reason: 'unsupportedChip',
        detail: 'ESP32-C3 is not an ESP32-S3.',
      }),
    )
    expect(commandsOf(refusedChip)).toEqual([
      sent(
        refused(1, 'unsupportedChip', 'ESP32-C3 is not an ESP32-S3.'),
        ...backToConsole,
      ),
    ])
  })
})

describe('ops on a hold', () => {
  test.each([read, write, digest, reboot])(
    '%o with nothing held is notEntered',
    op => {
      expect(commandsOf(step(serialLive(), opMessage(7, op)))).toEqual([
        sent(refused(7, 'notEntered')),
      ])
    },
  )

  test('an op under a stale lease is notEntered', () => {
    expect(
      commandsOf(
        step(
          held({ state: flashing({ lease: otherLease }) }),
          opMessage(7, read),
        ),
      ),
    ).toEqual([sent(refused(7, 'notEntered'))])
  })

  test('one op waits behind the running one, a third is busy', () => {
    const two = step(held({ ops: [queued(1, read)] }), opMessage(2, digest))
    expect(commandsOf(two)).toEqual([])
    expect(portOf(two.model).ops).toHaveLength(2)
    expect(commandsOf(step(two.model, opMessage(3, digest)))).toEqual([
      sent(refused(3, 'busy')),
    ])
  })

  test('a finished op is answered with its outcome and the next one runs', () => {
    const finished = step(
      held({ ops: [queued(1, read), queued(2, digest)] }),
      finish(1, { _tag: 'Read', md5 }),
    )
    expect(commandsOf(finished)).toEqual([
      sent(result(1, { _tag: 'Read', md5 })),
    ])
    expect(runningOp(finished.model)).toEqual(
      Option.some({ attempt: 0, id: requestId(2), op: digest }),
    )
    expect(portOf(finished.model).activity).toMatchObject([
      { kind: 'Read', label: Option.some('ota_0'), result: 'done' },
    ])
  })

  test('progress shows on the running op only', () => {
    const before = held({ ops: [queued(1, read)] })
    const moved = step(
      before,
      Message.ProgressedOp({
        attempt: 0,
        id: requestId(1),
        done: 512,
        total: 0x200000,
      }),
    )
    expect(portOf(moved.model).ops[0]).toMatchObject({ done: 512 })
    expect(
      step(
        before,
        Message.ProgressedOp({
          attempt: 0,
          id: requestId(9),
          done: 1,
          total: 1,
        }),
      ).model,
    ).toBe(before)
  })

  test('a result from an earlier hold is ignored', () => {
    const before = held({ ops: [queued(1, read)] })
    expect(step(before, finish(1, { _tag: 'Read', md5 }, 7)).model).toBe(before)
  })

  test('a payload that arrived damaged keeps the hold; flash was never touched', () => {
    const failed = step(
      held({ ops: [queued(1, write), queued(2, digest)] }),
      finish(1, { _tag: 'Failed', reason: 'payloadMismatch', detail: '' }),
    )
    expect(commandsOf(failed)).toEqual([sent(refused(1, 'payloadMismatch'))])
    expect(portOf(failed.model).state._tag).toBe('Flashing')
    expect(portOf(failed.model).ops).toEqual([queued(2, digest)])
  })

  test('a chip error ends the hold and the rest were never entered', () => {
    const failed = step(
      held({ ops: [queued(1, write), queued(2, digest)] }),
      finish(1, { _tag: 'Failed', reason: 'chipError', detail: 'timeout' }),
    )
    expect(commandsOf(failed)).toEqual([
      sent(
        refused(1, 'chipError', 'timeout'),
        refused(2, 'notEntered'),
        ...backToConsole,
      ),
    ])
    expect(portRequirement(failed.model)).toMatchObject(
      Option.some({ _tag: 'Console', attempt: 1 }),
    )
  })

  test('the port lost mid-op fails it portLost and goes back to the console', () => {
    const lost = step(
      held({ ops: [queued(1, read), queued(2, digest)] }),
      Message.LostPort({ attempt: 0 }),
    )
    expect(commandsOf(lost)).toEqual([
      sent(refused(1, 'portLost'), refused(2, 'notEntered'), ...backToConsole),
    ])
    expect(portOf(lost.model).activity).toMatchObject([
      { kind: 'Read', result: 'stopped' },
    ])
  })
  test('the port lost during a Reboot is the reset taking effect: Rebooting, then the console', () => {
    const lost = step(
      held({ ops: [queued(1, reboot)] }),
      Message.LostPort({ attempt: 0 }),
    )
    expect(commandsOf(lost)).toEqual([
      sent(result(1, { _tag: 'Rebooting' }), ...backToConsole),
    ])
  })
})

describe('leaving update mode', () => {
  test('Release with nothing held is Released', () => {
    const console = serialLive()
    const released = step(console, opMessage(4, { _tag: 'Release', lease }))
    expect(commandsOf(released)).toEqual([
      sent(result(4, { _tag: 'Released' })),
    ])
    expect(portOf(released.model).state).toEqual(portOf(console).state)
  })

  test('Release mid-op stops the head, the queued op was never entered, and the console comes back', () => {
    const released = step(
      held({ ops: [queued(1, write), queued(2, digest)], writesAllowed: true }),
      opMessage(3, { _tag: 'Release', lease: otherLease }),
    )
    expect(commandsOf(released)).toEqual([
      sent(
        result(3, { _tag: 'Released' }),
        refused(1, 'stopped'),
        refused(2, 'notEntered'),
        ...backToConsole,
      ),
    ])
    expect(portOf(released.model)).toMatchObject({
      attempt: 1,
      state: { _tag: 'Reacquiring' },
      ops: [],
    })
    expect(runningOp(released.model)).toEqual(Option.none())
  })

  test('Reboot goes back to the console; a queued Enter starts a new hold once it is open, other ops are notEntered', () => {
    const rebooted = step(
      held({ ops: [queued(1, reboot), queued(2, enter)] }),
      finish(1, { _tag: 'Rebooting' }),
    )
    expect(commandsOf(rebooted)).toEqual([
      sent(result(1, { _tag: 'Rebooting' }), ...backToConsole),
    ])
    expect(portOf(rebooted.model).ops).toEqual([queued(2, enter)])
    expect(portRequirement(rebooted.model)).toMatchObject(
      Option.some({ _tag: 'Console', attempt: 1 }),
    )
    const opened = step(rebooted.model, Message.OpenedPort())
    expect(portOf(opened.model).state).toMatchObject({
      _tag: 'Entering',
      lease,
    })
    expect(portOf(opened.model).epoch).toBe(epoch(2))

    const others = step(
      held({ ops: [queued(1, reboot), queued(2, read)] }),
      finish(1, { _tag: 'Rebooting' }),
    )
    expect(commandsOf(others)).toEqual([
      sent(
        result(1, { _tag: 'Rebooting' }),
        refused(2, 'notEntered'),
        ...backToConsole,
      ),
    ])
  })

  test('an Enter waiting for a console that never comes back is entryFailed', () => {
    const failed = step(
      serialLive({
        attempt: 1,
        state: { _tag: 'Reacquiring' },
        ops: [queued(2, enter)],
      }),
      Message.FailedPort({ reason: 'reacquireTimeout' }),
    )
    expect(commandsOf(failed)).toEqual([
      sent(
        changed(1, { _tag: 'Failed', reason: 'reacquireTimeout' }),
        refused(2, 'entryFailed'),
      ),
    ])
  })

  test('Restart device is offered only while update mode is idle, and leaves it like a Release', () => {
    const busy = held({ ops: [queued(1, read)] })
    expect(step(busy, Message.ClickedRestartDevice()).model).toBe(busy)
    const restarted = step(held(), Message.ClickedRestartDevice())
    expect(commandsOf(restarted)).toEqual([sent(...backToConsole)])
    expect(portOf(restarted.model).activity).toMatchObject([
      { kind: 'Restart', result: 'done' },
    ])
  })
})

const presence = (present: boolean) =>
  Message.ReceivedFromRelay({
    message: { _tag: 'PeerPresence', role: 'agent', present },
  })

describe('the agent resending', () => {
  test('a resend of the running op joins it: no second run, no answer until it finishes', () => {
    const running = held({ ops: [queued(5, write)], writesAllowed: true })
    const resend = step(running, opMessage(5, write))
    expect(commandsOf(resend)).toEqual([])
    expect(portOf(resend.model).ops).toEqual([queued(5, write)])
    expect(runningOp(resend.model)).toEqual(runningOp(running))
    expect(
      commandsOf(step(resend.model, finish(5, { _tag: 'Written', md5 }))),
    ).toEqual([sent(result(5, { _tag: 'Written', md5 }))])
  })

  test('an op queued behind the running one hears heartbeats, so it never stalls behind it', () => {
    const behind = held({ ops: [queued(5, read), queued(6, digest)] })
    expect(waitingOps(behind)).toEqual([
      { id: requestId(6), total: totalOf(digest) },
    ])
    const resend = step(behind, opMessage(6, digest))
    expect(commandsOf(resend)).toEqual([])
    expect(portOf(resend.model).ops).toHaveLength(2)
  })

  test('a resend of an op answered while its answer was lost is answered again, not run again', () => {
    const answered = step(
      held({ ops: [queued(5, digest)] }),
      finish(5, { _tag: 'Digest', md5 }),
    ).model
    const resend = step(answered, opMessage(5, digest))
    expect(commandsOf(resend)).toEqual([
      sent(result(5, { _tag: 'Digest', md5 })),
    ])
    expect(portOf(resend.model).ops).toEqual([])

    const entered = step(
      held({ ops: [queued(7, enter)] }),
      finish(7, { _tag: 'Entered' }),
    ).model
    expect(commandsOf(step(entered, opMessage(7, enter)))).toEqual([
      { name: SendEntered.name, args: { id: requestId(7) } },
    ])
  })

  test('the same id under another lease is a new op', () => {
    const before = held({ ops: [queued(5, read)] })
    expect(
      commandsOf(step(before, opMessage(5, { ...read, lease: otherLease }))),
    ).toEqual([sent(refused(5, 'notEntered'))])
  })
})

describe('an unattended hold', () => {
  test('the agent leaving never stops a running op; the hold ends 30 s after it finished', () => {
    const away = step(held({ ops: [queued(1, read)] }), presence(false)).model
    expect(holdExpiryOf(away)).toEqual(Option.none())
    expect(
      step(
        away,
        Message.ExpiredHold({ attempt: 0, lease, after: AGENT_GRACE_MS }),
      ).model,
    ).toBe(away)
    const done = step(away, finish(1, { _tag: 'Read', md5 })).model
    expect(holdExpiryOf(done)).toEqual(
      Option.some({ attempt: 0, lease, after: AGENT_GRACE_MS }),
    )
    story(
      step,
      given(done),
      message(
        Message.ExpiredHold({ attempt: 0, lease, after: AGENT_GRACE_MS }),
      ),
      Command.resolve(
        SendControl({ messages: backToConsole }),
        Message.CompletedSendControl(),
      ),
      model(next => {
        expect(portOf(next).state._tag).toBe('Reacquiring')
      }),
    )
  })

  test('an idle hold whose Release was lost gives the device back after IDLE_HOLD_MS', () => {
    const idle = held()
    expect(holdExpiryOf(idle)).toEqual(
      Option.some({ attempt: 0, lease, after: IDLE_HOLD_MS }),
    )
    expect(
      commandsOf(
        step(
          idle,
          Message.ExpiredHold({ attempt: 0, lease, after: IDLE_HOLD_MS }),
        ),
      ),
    ).toEqual([sent(...backToConsole)])
  })

  test('a write waiting for the visitor waits as long as the agent is here', () => {
    const asking = held({ ops: [queued(1, write)] })
    expect(holdExpiryOf(asking)).toEqual(Option.none())
    expect(holdExpiryOf(step(asking, presence(false)).model)).toEqual(
      Option.some({ attempt: 0, lease, after: AGENT_GRACE_MS }),
    )
  })

  test('an expiry from before the hold changed is ignored', () => {
    const idle = held()
    expect(
      step(
        idle,
        Message.ExpiredHold({
          attempt: 0,
          lease: otherLease,
          after: IDLE_HOLD_MS,
        }),
      ).model,
    ).toBe(idle)
  })

  test('a 30 s expiry that lands as the agent returns keeps the hold for IDLE_HOLD_MS', () => {
    const idle = held()
    expect(
      step(
        idle,
        Message.ExpiredHold({ attempt: 0, lease, after: AGENT_GRACE_MS }),
      ).model,
    ).toBe(idle)
  })
})

describe('the visitor allowing writes', () => {
  test('the first Write waits for the visitor; Allow runs it and lasts the session', () => {
    const asking = held({ ops: [queued(1, write)] })
    expect(runningOp(asking)).toEqual(Option.none())
    expect(waitingOps(asking)).toEqual([{ id: requestId(1), total: 0x1e0000 }])
    const allowed = step(asking, Message.ClickedAllowWrites())
    expect(liveOf(allowed.model).writesAllowed).toBe(true)
    expect(commandsOf(allowed)).toEqual([
      {
        name: SaveSession.name,
        args: {
          stored: expect.objectContaining({ writesAllowed: true }),
        },
      },
    ])
    expect(runningOp(allowed.model)).toEqual(
      Option.some({ attempt: 0, id: requestId(1), op: write }),
    )

    const next = step(
      step(allowed.model, finish(1, { _tag: 'Written', md5 })).model,
      opMessage(2, write),
    )
    expect(runningOp(next.model)).toEqual(
      Option.some({ attempt: 0, id: requestId(2), op: write }),
    )
  })

  test('Not now refuses the write notAllowed and update mode goes on', () => {
    const declined = step(
      held({ ops: [queued(1, write), queued(2, digest)] }),
      Message.ClickedDeclineWrite(),
    )
    expect(commandsOf(declined)).toEqual([sent(refused(1, 'notAllowed'))])
    expect(portOf(declined.model).ops).toEqual([queued(2, digest)])
    expect(portOf(declined.model).activity).toMatchObject([
      { kind: 'Write', result: 'declined', label: Option.some('ota_1') },
    ])
    expect(liveOf(declined.model).writesAllowed).toBe(false)
  })

  test('reads, digests, enter and reboot never ask', () => {
    for (const op of [read, digest, enter, reboot]) {
      expect(Option.isSome(runningOp(held({ ops: [queued(1, op)] })))).toBe(
        true,
      )
    }
  })
})

describe('Disconnect during a write', () => {
  test('asks first, and stops only on the second answer', () => {
    story(
      step,
      given(held({ ops: [queued(1, write)], writesAllowed: true })),
      message(Message.ClickedDisconnect()),
      Command.expectNone(),
      model(next => {
        expect(liveOf(next).confirmingStop).toBe(true)
      }),
      message(Message.ClickedKeepGoing()),
      model(next => {
        expect(liveOf(next).confirmingStop).toBe(false)
      }),
      message(Message.ClickedDisconnect()),
      message(Message.ClickedStopAndDisconnect()),
      Command.resolve(SayBye, Message.SaidBye()),
      Command.resolve(ClearSession, Message.CompletedClearSession()),
      model(next => {
        expect(next.page).toEqual(Page.Ended({ reason: 'visitorDisconnected' }))
        expect(portRequirement(next)).toEqual(Option.none())
      }),
    )
  })

  test('outside a write it disconnects at once', () => {
    expect(
      commandsOf(
        step(held({ ops: [queued(1, read)] }), Message.ClickedDisconnect()),
      ),
    ).toEqual([{ name: SayBye.name, args: undefined }])
  })
})
