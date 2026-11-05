import { describe, expect, test } from 'bun:test'
import * as Option from 'effect/Option'
import {
  type AnyCommand,
  Command,
  given,
  message,
  model,
  story,
} from 'foldkit/story'

import { type BrowserMsg, Lease, type Op } from '@cycling/protocol'

import {
  ChooseSerial,
  ChooseSerialAgain,
  ChooseUsb,
  ClearSession,
  MintSession,
  SaveSession,
  SayBye,
  SendControl,
  WaitRelayRetry,
} from './command.ts'
import { Message } from './message.ts'
import {
  type Flags,
  type Live,
  type Model,
  type Port,
  Page,
  Relay,
  Source,
} from './model.ts'
import { relayRequirement, serialRequirement } from './resources.ts'
import {
  baud,
  descriptor,
  epoch,
  idle,
  identity,
  nextSession,
  otherIdentity,
  requestId,
  serialLive,
  session,
  usbLive,
} from './test/fixtures.ts'
import { init, update } from './update.ts'

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

const commandsOf = (result: {
  readonly commands?: ReadonlyArray<AnyCommand>
}) => (result.commands ?? []).map(({ name, args }) => ({ name, args }))

const control = (...messages: ReadonlyArray<BrowserMsg>) => ({
  name: 'SendControl',
  args: { messages },
})

const welcomeFlags = (patch: Partial<Flags['environment']> = {}): Flags => ({
  environment: {
    hasWebSerial: true,
    hasWebUsb: true,
    isSecureContext: true,
    relayConfigured: true,
    ...patch,
  },
  stored: Option.none(),
})

describe('connect flow', () => {
  test('Idle to a joined, open port, command by command', () => {
    const stored = { session, identity, baud: baud(115_200), epoch: epoch(0) }
    story(
      update,
      given(idle),
      message(Message.ClickedConnectDevice()),
      Command.expectExact(ChooseSerial),
      model(next => {
        expect(next.page).toEqual(Page.Choosing({ kind: 'serial' }))
      }),
      Command.resolve(ChooseSerial, Message.ChoseSerial({ identity, session })),
      Command.resolve(SaveSession({ stored }), Message.CompletedSaveSession()),
      model(next => {
        expect(liveOf(next).relay).toEqual(
          Relay.Dialing({ attempt: 0, failures: 0 }),
        )
        expect(portOf(next).state).toEqual({ _tag: 'Opening' })
      }),
      message(Message.JoinedRelay()),
      Command.resolve(
        SendControl({
          messages: [
            {
              _tag: 'DeviceAnnounced',
              device: { _tag: 'serial', identity },
            },
            {
              _tag: 'PortChanged',
              status: { epoch: epoch(0), state: { _tag: 'Opening' } },
            },
          ],
        }),
        Message.CompletedSendControl(),
      ),
      message(Message.OpenedPort()),
      Command.resolve(
        SaveSession({ stored: { ...stored, epoch: epoch(1) } }),
        Message.CompletedSaveSession(),
      ),
      Command.resolve(
        SendControl({
          messages: [
            {
              _tag: 'PortChanged',
              status: {
                epoch: epoch(1),
                state: { _tag: 'Open', baud: 115_200 },
              },
            },
          ],
        }),
        Message.CompletedSendControl(),
      ),
      model(next => {
        expect(portOf(next).epoch).toBe(epoch(1))
        expect(liveOf(next).relay).toEqual(
          Relay.Joined({ attempt: 0, agentPresent: false }),
        )
      }),
    )
  })

  test('a cancelled chooser returns to Idle without a notice', () => {
    story(
      update,
      given(idle),
      message(Message.ClickedConnectDevice()),
      Command.resolve(ChooseSerial, Message.CancelledChooser()),
      model(next => {
        expect(next.page).toEqual(Page.Idle({ notice: Option.none() }))
      }),
    )
  })

  test('a failed chooser returns to Idle with the reason', () => {
    story(
      update,
      given(idle),
      message(Message.ClickedConnectDevice()),
      Command.resolve(
        ChooseSerial,
        Message.FailedChooser({ reason: 'no ids' }),
      ),
      model(next => {
        expect(next.page).toEqual(Page.Idle({ notice: Option.some('no ids') }))
      }),
    )
  })

  test('the describe-only link joins without a port and announces the descriptor', () => {
    story(
      update,
      given(idle),
      message(Message.ClickedDescribeUsb()),
      Command.expectExact(ChooseUsb),
      Command.resolve(ChooseUsb, Message.ChoseUsb({ descriptor, session })),
      model(next => {
        expect(Source.guards.Usb(liveOf(next).source)).toBe(true)
        expect(serialRequirement(next)).toEqual(Option.none())
        expect(Option.isSome(relayRequirement(next))).toBe(true)
      }),
      message(Message.JoinedRelay()),
      Command.resolve(
        SendControl({
          messages: [
            { _tag: 'DeviceAnnounced', device: { _tag: 'usb', descriptor } },
          ],
        }),
        Message.CompletedSendControl(),
      ),
    )
  })
})

describe('port lifecycle', () => {
  test('a lost port waits for the same device and opens again under a new epoch', () => {
    const lost = update(
      serialLive({ attempt: 0, epoch: 1 }),
      Message.LostPort({ attempt: 0 }),
    )
    expect(portOf(lost.model)).toMatchObject({
      attempt: 1,
      epoch: 1,
      state: { _tag: 'Reacquiring' },
    })
    expect(commandsOf(lost)).toEqual([
      control(
        {
          _tag: 'PortChanged',
          status: { epoch: epoch(1), state: { _tag: 'Lost' } },
        },
        {
          _tag: 'PortChanged',
          status: { epoch: epoch(1), state: { _tag: 'Reacquiring' } },
        },
      ),
    ])
    expect(serialRequirement(lost.model)).toEqual(
      Option.some({ identity, baud: 115_200, attempt: 1 }),
    )

    const reopened = update(lost.model, Message.OpenedPort())
    expect(portOf(reopened.model)).toMatchObject({
      epoch: 2,
      state: { _tag: 'Open', baud: 115_200 },
    })
    expect(commandsOf(reopened)).toEqual([
      control({
        _tag: 'PortChanged',
        status: { epoch: epoch(2), state: { _tag: 'Open', baud: 115_200 } },
      }),
      {
        name: SaveSession.name,
        args: {
          stored: { session, identity, baud: 115_200, epoch: epoch(2) },
        },
      },
    ])
  })

  test('a late LostPort from a replaced stream is ignored', () => {
    const reopened = serialLive({ attempt: 2 })
    const stale = update(reopened, Message.LostPort({ attempt: 1 }))
    expect(stale.model).toBe(reopened)
    expect(commandsOf(stale)).toEqual([])
  })

  test('a port that never comes back fails, and choosing again opens the new one', () => {
    const failed = update(
      serialLive({ attempt: 1, state: { _tag: 'Reacquiring' } }),
      Message.FailedPort({ reason: 'reacquireTimeout' }),
    )
    expect(portOf(failed.model).state).toEqual({
      _tag: 'Failed',
      reason: 'reacquireTimeout',
    })
    expect(serialRequirement(failed.model)).toEqual(Option.none())

    const chooser = update(failed.model, Message.ClickedChooseAgain())
    expect(commandsOf(chooser)).toEqual([
      { name: ChooseSerialAgain.name, args: undefined },
    ])

    const chosen = update(
      failed.model,
      Message.ChoseSerialAgain({ identity: otherIdentity }),
    )
    expect(portOf(chosen.model)).toMatchObject({
      identity: otherIdentity,
      attempt: 2,
      state: { _tag: 'Opening' },
    })
    expect(commandsOf(chosen).map(({ name }) => name)).toEqual([
      'SendControl',
      'SaveSession',
    ])
  })

  test('choosing again while the port is healthy does nothing', () => {
    const healthy = serialLive()
    expect(update(healthy, Message.ClickedChooseAgain()).model).toBe(healthy)
  })
})

describe('relay lifecycle', () => {
  test('a dropped socket backs off with growing delays and redials under a new attempt', () => {
    const dropped = update(
      serialLive({ relay: Relay.Joined({ attempt: 0, agentPresent: true }) }),
      Message.ClosedRelay({ attempt: 0, code: 1006 }),
    )
    expect(liveOf(dropped.model).relay).toEqual(
      Relay.Backoff({ attempt: 0, failures: 1 }),
    )
    expect(commandsOf(dropped)).toEqual([
      { name: WaitRelayRetry.name, args: { attempt: 0, delayMs: 500 } },
    ])
    expect(relayRequirement(dropped.model)).toEqual(Option.none())

    const redialing = update(
      dropped.model,
      Message.RetryRelayDue({ attempt: 0 }),
    )
    expect(liveOf(redialing.model).relay).toEqual(
      Relay.Dialing({ attempt: 1, failures: 1 }),
    )
    expect(
      Option.map(relayRequirement(redialing.model), ({ attempt }) => attempt),
    ).toEqual(Option.some(1))

    const failedAgain = update(redialing.model, Message.FailedDialRelay())
    expect(liveOf(failedAgain.model).relay).toEqual(
      Relay.Backoff({ attempt: 1, failures: 2 }),
    )
    expect(commandsOf(failedAgain)).toEqual([
      { name: WaitRelayRetry.name, args: { attempt: 1, delayMs: 1000 } },
    ])
  })

  test('a retry timer from an earlier attempt does nothing', () => {
    const waiting = serialLive({
      relay: Relay.Backoff({ attempt: 3, failures: 1 }),
    })
    expect(update(waiting, Message.RetryRelayDue({ attempt: 2 })).model).toBe(
      waiting,
    )
  })

  test('code in use mints a fresh session and dials it', () => {
    const inUse = update(
      serialLive({ relay: Relay.Joined({ attempt: 0, agentPresent: false }) }),
      Message.ClosedRelay({ attempt: 0, code: 4409 }),
    )
    expect(commandsOf(inUse)).toEqual([
      { name: MintSession.name, args: undefined },
    ])
    expect(relayRequirement(inUse.model)).toEqual(Option.none())

    const minted = update(
      inUse.model,
      Message.MintedSession({ session: nextSession }),
    )
    expect(liveOf(minted.model).session).toEqual(nextSession)
    expect(liveOf(minted.model).relay).toEqual(
      Relay.Dialing({ attempt: 1, failures: 0 }),
    )
    expect(commandsOf(minted)).toEqual([
      {
        name: SaveSession.name,
        args: {
          stored: {
            session: nextSession,
            identity,
            baud: 115_200,
            epoch: epoch(1),
          },
        },
      },
    ])
  })

  test('being replaced by another tab ends the session and clears the stored one', () => {
    const replaced = update(
      serialLive(),
      Message.ClosedRelay({ attempt: 0, code: 4410 }),
    )
    expect(replaced.model.page).toEqual(Page.Ended({ reason: 'replaced' }))
    expect(commandsOf(replaced)).toEqual([
      { name: ClearSession.name, args: undefined },
    ])
    expect(serialRequirement(replaced.model)).toEqual(Option.none())
    expect(relayRequirement(replaced.model)).toEqual(Option.none())
  })

  test.each([
    [4000, 'visitorDisconnected'],
    [4001, 'visitorGone'],
    [4408, 'expired'],
    [4401, 'unauthorized'],
    [4404, 'notFound'],
    [4426, 'unsupportedVersion'],
    [4429, 'rateLimited'],
    [1008, 'protocolViolation'],
  ] as const)(
    'close %d ends the page as %s and clears the stored session',
    (code, reason) => {
      const closed = update(
        serialLive(),
        Message.ClosedRelay({ attempt: 0, code }),
      )
      expect(closed.model.page).toEqual(Page.Ended({ reason }))
      expect(commandsOf(closed)).toEqual([
        { name: ClearSession.name, args: undefined },
      ])
      expect(relayRequirement(closed.model)).toEqual(Option.none())
    },
  )

  test('a close from an earlier socket does nothing', () => {
    const joined = serialLive({
      relay: Relay.Joined({ attempt: 2, agentPresent: true }),
    })
    expect(
      update(joined, Message.ClosedRelay({ attempt: 1, code: 4408 })).model,
    ).toBe(joined)
  })

  test('the agent arriving and leaving shows as developer presence', () => {
    const waiting = serialLive({
      relay: Relay.Joined({ attempt: 0, agentPresent: false }),
    })
    const arrived = update(
      waiting,
      Message.ReceivedFromRelay({
        message: { _tag: 'PeerPresence', role: 'agent', present: true },
      }),
    )
    expect(liveOf(arrived.model).relay).toEqual(
      Relay.Joined({ attempt: 0, agentPresent: true }),
    )
    const other = update(
      arrived.model,
      Message.ReceivedFromRelay({
        message: { _tag: 'PeerPresence', role: 'browser', present: false },
      }),
    )
    expect(other.model).toBe(arrived.model)
  })
})

describe('agent ops', () => {
  const lease = Lease.make('0123456789abcdef')
  const op = (body: Op) =>
    Message.ReceivedFromRelay({
      message: { _tag: 'Op', id: requestId(5), op: body },
    })

  test('an op the page cannot run is refused notEntered, and a Release is always answered', () => {
    expect(
      commandsOf(update(serialLive(), op({ _tag: 'Enter', lease }))),
    ).toEqual([
      control({
        _tag: 'OpResult',
        id: requestId(5),
        outcome: { _tag: 'Failed', reason: 'notEntered', detail: '' },
      }),
    ])
    expect(
      commandsOf(update(usbLive(), op({ _tag: 'Release', lease }))),
    ).toEqual([
      control({
        _tag: 'OpResult',
        id: requestId(5),
        outcome: { _tag: 'Released' },
      }),
    ])
  })
})

describe('disconnect', () => {
  test('says Bye while both handles are held, then releases both', () => {
    const joined = serialLive()
    const leaving = update(joined, Message.ClickedDisconnect())
    expect(commandsOf(leaving)).toEqual([
      { name: SayBye.name, args: undefined },
    ])
    expect(Option.isSome(serialRequirement(leaving.model))).toBe(true)
    expect(Option.isSome(relayRequirement(leaving.model))).toBe(true)

    const done = update(leaving.model, Message.SaidBye())
    expect(done.model.page).toEqual(
      Page.Ended({ reason: 'visitorDisconnected' }),
    )
    expect(commandsOf(done)).toEqual([
      { name: ClearSession.name, args: undefined },
    ])
    expect(serialRequirement(done.model)).toEqual(Option.none())
    expect(relayRequirement(done.model)).toEqual(Option.none())
  })

  test('ends at once when the relay is not joined', () => {
    const ended = update(
      serialLive({ relay: Relay.Backoff({ attempt: 0, failures: 2 }) }),
      Message.ClickedDisconnect(),
    )
    expect(ended.model.page).toEqual(
      Page.Ended({ reason: 'visitorDisconnected' }),
    )
  })

  test('Start over returns to Idle', () => {
    const ended: Model = {
      canDescribeUsb: true,
      page: Page.Ended({ reason: 'visitorDisconnected' }),
    }
    expect(update(ended, Message.ClickedStartOver()).model.page).toEqual(
      Page.Idle({ notice: Option.none() }),
    )
  })
})

describe('init', () => {
  test('a stored session resumes the same code with no gesture', () => {
    const resumed = init({
      ...welcomeFlags(),
      stored: Option.some({
        session,
        identity,
        baud: baud(921_600),
        epoch: epoch(4),
      }),
    })
    expect(liveOf(resumed.model).session).toEqual(session)
    expect(portOf(resumed.model)).toMatchObject({
      identity,
      baud: 921_600,
      attempt: 1,
      epoch: 4,
      state: { _tag: 'Reacquiring' },
    })
    expect(
      portOf(update(resumed.model, Message.OpenedPort()).model).epoch,
    ).toBe(epoch(5))
    expect(liveOf(resumed.model).relay).toEqual(
      Relay.Dialing({ attempt: 1, failures: 0 }),
    )
    expect(serialRequirement(resumed.model)).toEqual(
      Option.some({ identity, baud: 921_600, attempt: 1 }),
    )
    expect(commandsOf(resumed)).toEqual([])
  })

  test('with nothing stored the page is Idle', () => {
    expect(init(welcomeFlags()).model.page).toEqual(
      Page.Idle({ notice: Option.none() }),
    )
  })

  test.each([
    [{ isSecureContext: false }, 'secureContext'],
    [{ hasWebSerial: false }, 'webSerial'],
    [{ relayConfigured: false }, 'relayUrl'],
  ] as const)(
    '%o is Unsupported(%s) even with a stored session',
    (patch, missing) => {
      const flags = {
        ...welcomeFlags(patch),
        stored: Option.some({
          session,
          identity,
          baud: baud(115_200),
          epoch: epoch(0),
        }),
      }
      expect(init(flags).model.page).toEqual(Page.Unsupported({ missing }))
    },
  )

  test('the describe-only link is offered only where WebUSB exists', () => {
    expect(init(welcomeFlags({ hasWebUsb: false })).model.canDescribeUsb).toBe(
      false,
    )
    expect(init(welcomeFlags()).model.canDescribeUsb).toBe(true)
  })
})

describe('traffic', () => {
  test('a batch adds a row and the running counts', () => {
    const next = update(
      serialLive(),
      Message.Traffic({
        direction: 'fromDevice',
        bytes: Uint8Array.of(0x68, 0x69, 0x0a),
        dropped: 0,
      }),
    )
    expect(liveOf(next.model).monitor).toMatchObject({
      bytesFromDevice: 3,
      bytesToDevice: 0,
      rows: [{ direction: 'fromDevice', hex: '68 69 0a', text: 'hi·' }],
    })
  })

  test('traffic after the session ended is ignored', () => {
    const ended: Model = {
      canDescribeUsb: true,
      page: Page.Ended({ reason: 'visitorDisconnected' }),
    }
    expect(
      update(
        ended,
        Message.Traffic({
          direction: 'toDevice',
          bytes: Uint8Array.of(1),
          dropped: 0,
        }),
      ).model,
    ).toBe(ended)
  })
})

describe('copy code', () => {
  test('marks the code copied once the clipboard accepts it', () => {
    const copied = update(serialLive(), Message.CopiedCode())
    expect(liveOf(copied.model).copied).toBe(true)
  })
})
