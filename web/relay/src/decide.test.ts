import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Schema from 'effect/Schema'

import {
  type AgentMsg,
  type BrowserMsg,
  CloseCode,
  type Device,
  encodeAgentMsg,
  encodeBrowserMsg,
  Epoch,
  Lease,
  type PortStatus,
  RequestId,
} from '@cycling/protocol'

import { Policy } from './Policy.ts'
import type { SessionRecord } from './SessionRecord.ts'
import { classify } from './classify.ts'
import {
  type Action,
  type Audience,
  decide,
  type Event,
  nextDeadline,
  type Presence,
  type State,
} from './decide.ts'

const MINUTE = 60_000
const T0 = 1_000_000_000_000
const hash = 'a'.repeat(64)
const otherHash = 'b'.repeat(64)

const device: Device = {
  _tag: 'serial',
  identity: { vendorId: 0x303a, productId: 0x1001 },
}
const port: PortStatus = {
  epoch: Schema.decodeSync(Epoch)(1),
  state: { _tag: 'Open', baud: 115200 },
}

const nobody: Presence = { browser: false, agent: false }
const onlyBrowser: Presence = { browser: true, agent: false }
const onlyAgent: Presence = { browser: false, agent: true }
const both: Presence = { browser: true, agent: true }

const open = (patch: Partial<SessionRecord> = {}): SessionRecord => ({
  _tag: 'Open',
  ownerKeyHash: hash,
  createdAt: T0,
  expiresAt: T0 + Policy.openTtlMs,
  browserGoneAt: null,
  device: null,
  port: null,
  ...patch,
})
const paired = (patch: Partial<SessionRecord> = {}): SessionRecord => ({
  ...open(),
  _tag: 'Paired',
  expiresAt: T0 + Policy.pairedTtlMs,
  ...patch,
})

const state = (record: SessionRecord | null, presence: Presence): State => ({
  record,
  presence,
})

const subject: Audience = { _tag: 'Subject' }
const everyone: Audience = { _tag: 'Everyone' }
const others = (role: 'browser' | 'agent'): Audience => ({
  _tag: 'Others',
  role,
})

const close = (who: Audience, code: CloseCode): Action => ({
  _tag: 'Close',
  who,
  code,
})

const types = (actions: ReadonlyArray<Action>) =>
  actions.map(action => action._tag)

const run = (s: State, event: Event, now = T0) => decide(s, event, now)

const browserSays = (message: BrowserMsg): Event => ({
  _tag: 'BrowserText',
  message,
  text: encodeBrowserMsg(message),
})
const agentSays = (message: AgentMsg): Event => ({
  _tag: 'AgentText',
  message,
  text: encodeAgentMsg(message),
})
const forwarded = (to: 'browser' | 'agent', text: string): Action => ({
  _tag: 'Forward',
  to,
  text,
})

describe('browser upgrade', () => {
  test('claims an unclaimed code and stays open for the open TTL', () => {
    const result = run(state(null, nobody), {
      _tag: 'BrowserUpgrade',
      ownerKeyHash: hash,
    })
    expect(result.record).toEqual(open())
    expect(result.actions).toEqual([
      {
        _tag: 'Send',
        to: subject,
        message: {
          _tag: 'Welcome',
          role: 'browser',
          snapshot: {
            device: null,
            port: null,
            expiresAt: T0 + Policy.openTtlMs,
            browserPresent: true,
            agentPresent: false,
          },
        },
      },
      {
        _tag: 'Send',
        to: others('agent'),
        message: { _tag: 'PeerPresence', role: 'browser', present: true },
      },
      { _tag: 'SetAlarm', at: T0 + Policy.openTtlMs },
    ])
  })

  test('resumes with the same key and tells the agent the browser is back', () => {
    const gone = paired({ browserGoneAt: T0 + MINUTE })
    const result = run(
      state(gone, onlyAgent),
      { _tag: 'BrowserUpgrade', ownerKeyHash: hash },
      T0 + 2 * MINUTE,
    )
    expect(result.record).toEqual(paired({ browserGoneAt: null }))
    expect(types(result.actions)).toEqual(['Send', 'Send', 'SetAlarm'])
    expect(result.actions[0]).toMatchObject({
      message: { snapshot: { browserPresent: true, agentPresent: true } },
    })
  })

  test('closes an older browser socket with replaced when the same key returns', () => {
    const result = run(state(open(), onlyBrowser), {
      _tag: 'BrowserUpgrade',
      ownerKeyHash: hash,
    })
    expect(result.actions[0]).toEqual(
      close(others('browser'), CloseCode.replaced),
    )
  })

  test('refuses a different key with codeInUse and leaves the record alone', () => {
    const record = paired()
    const result = run(state(record, both), {
      _tag: 'BrowserUpgrade',
      ownerKeyHash: otherHash,
    })
    expect(result.record).toBe(record)
    expect(result.actions).toEqual([close(subject, CloseCode.codeInUse)])
  })
})

describe('agent upgrade', () => {
  test('cannot create a session', () => {
    const result = run(state(null, nobody), { _tag: 'AgentUpgrade' })
    expect(result.record).toBeNull()
    expect(result.actions).toEqual([close(subject, CloseCode.notFound)])
  })

  test('pairs an open session and a late agent sees the stored device and port', () => {
    const result = run(
      state(open({ device, port }), onlyBrowser),
      { _tag: 'AgentUpgrade' },
      T0 + MINUTE,
    )
    expect(result.record).toEqual(
      paired({
        device,
        port,
        expiresAt: T0 + MINUTE + Policy.pairedTtlMs,
      }),
    )
    expect(result.actions[0]).toEqual({
      _tag: 'Send',
      to: subject,
      message: {
        _tag: 'Welcome',
        role: 'agent',
        snapshot: {
          device,
          port,
          expiresAt: T0 + MINUTE + Policy.pairedTtlMs,
          browserPresent: true,
          agentPresent: true,
        },
      },
    })
    expect(result.actions[1]).toEqual({
      _tag: 'Send',
      to: others('browser'),
      message: { _tag: 'PeerPresence', role: 'agent', present: true },
    })
  })

  test('replaces an older agent socket', () => {
    const result = run(state(paired(), both), { _tag: 'AgentUpgrade' })
    expect(result.actions[0]).toEqual(
      close(others('agent'), CloseCode.replaced),
    )
  })

  test('never extends a session past the hard cap', () => {
    const late = T0 + Policy.hardTtlMs - MINUTE
    const result = run(
      state(paired({ expiresAt: late }), both),
      { _tag: 'AgentUpgrade' },
      late - MINUTE,
    )
    expect(result.record?.expiresAt).toBe(T0 + Policy.hardTtlMs)
  })
})

describe('frames', () => {
  test('DeviceAnnounced is stored, forwarded and renews a paired session', () => {
    const result = run(
      state(paired(), both),
      browserSays({ _tag: 'DeviceAnnounced', device }),
      T0 + MINUTE,
    )
    expect(result.record).toEqual(
      paired({ device, expiresAt: T0 + MINUTE + Policy.pairedTtlMs }),
    )
    expect(result.actions).toEqual([
      forwarded('agent', encodeBrowserMsg({ _tag: 'DeviceAnnounced', device })),
    ])
  })

  test('PortChanged is stored and forwarded', () => {
    const message: BrowserMsg = { _tag: 'PortChanged', status: port }
    const result = run(state(open(), onlyBrowser), browserSays(message))
    expect(result.record).toEqual(open({ port }))
    expect(result.actions).toEqual([
      forwarded('agent', encodeBrowserMsg(message)),
    ])
  })

  test('an open session keeps its fixed deadline however chatty the browser is', () => {
    const result = run(
      state(open(), onlyBrowser),
      browserSays({ _tag: 'DeviceAnnounced', device }),
      T0 + 10 * MINUTE,
    )
    expect(result.record?.expiresAt).toBe(T0 + Policy.openTtlMs)
  })

  test('other browser results and every agent request are forwarded and renew', () => {
    const enter: AgentMsg = {
      _tag: 'Op',
      id: Schema.decodeSync(RequestId)(1),
      op: { _tag: 'Enter', lease: Lease.make('0123456789abcdef') },
    }
    const request = run(state(paired(), both), agentSays(enter), T0 + MINUTE)
    expect(request.actions).toEqual([
      forwarded('browser', encodeAgentMsg(enter)),
    ])
    expect(request.record?.expiresAt).toBe(T0 + MINUTE + Policy.pairedTtlMs)
  })

  test('renewal stops at the hard cap', () => {
    const near = T0 + Policy.hardTtlMs - MINUTE
    const renewed = run(
      state(paired(), both),
      browserSays({ _tag: 'DeviceAnnounced', device }),
      near,
    )
    expect(renewed.record?.expiresAt).toBe(T0 + Policy.hardTtlMs)
  })

  test('browser Bye ends the session for everyone', () => {
    const result = run(state(paired(), both), browserSays({ _tag: 'Bye' }))
    expect(result.record).toBeNull()
    expect(result.actions).toEqual([
      close(everyone, CloseCode.visitorDisconnected),
      { _tag: 'ClearAlarm' },
    ])
  })

  test('a malformed message closes only the sender', () => {
    const record = paired()
    const result = run(state(record, both), {
      _tag: 'Malformed',
      from: 'agent',
    })
    expect(result.record).toBe(record)
    expect(result.actions).toEqual([
      close(subject, CloseCode.protocolViolation),
    ])
  })

  test('a pong changes nothing', () => {
    const record = paired()
    expect(run(state(record, both), { _tag: 'Pong' })).toEqual({
      record,
      actions: [],
    })
  })

  test('a message arriving after the session ended closes the sender as unknown', () => {
    const result = run(
      state(null, onlyAgent),
      agentSays({
        _tag: 'Op',
        id: Schema.decodeSync(RequestId)(2),
        op: { _tag: 'Release', lease: Lease.make('0123456789abcdef') },
      }),
    )
    expect(result.record).toBeNull()
    expect(result.actions).toEqual([close(subject, CloseCode.notFound)])
  })
})

describe('closed sockets', () => {
  test('a replaced socket closing says nothing while its replacement lives', () => {
    const record = paired()
    expect(
      run(state(record, both), { _tag: 'Closed', role: 'browser' }),
    ).toEqual({ record, actions: [] })
  })

  test('the last agent leaving tells the browser and keeps the session paired', () => {
    const record = paired()
    const result = run(state(record, onlyBrowser), {
      _tag: 'Closed',
      role: 'agent',
    })
    expect(result.record).toBe(record)
    expect(result.actions).toEqual([
      {
        _tag: 'Send',
        to: others('browser'),
        message: { _tag: 'PeerPresence', role: 'agent', present: false },
      },
    ])
  })

  test('the last browser leaving starts the grace period', () => {
    const result = run(
      state(paired(), onlyAgent),
      { _tag: 'Closed', role: 'browser' },
      T0 + MINUTE,
    )
    expect(result.record).toEqual(paired({ browserGoneAt: T0 + MINUTE }))
    expect(result.actions).toEqual([
      {
        _tag: 'Send',
        to: others('agent'),
        message: { _tag: 'PeerPresence', role: 'browser', present: false },
      },
      { _tag: 'SetAlarm', at: T0 + MINUTE + Policy.browserGraceMs },
    ])
  })

  test('the grace period never outlasts the session', () => {
    const record = open({ expiresAt: T0 + 2 * MINUTE })
    const result = run(
      state(record, nobody),
      { _tag: 'Closed', role: 'browser' },
      T0 + MINUTE,
    )
    expect(result.actions.at(-1)).toEqual({
      _tag: 'SetAlarm',
      at: T0 + 2 * MINUTE,
    })
  })
})

describe('alarm', () => {
  const alarm = (s: State, now: number) => run(s, { _tag: 'Alarm' }, now)
  const endedWith = (code: CloseCode): ReadonlyArray<Action> => [
    close(everyone, code),
    { _tag: 'ClearAlarm' },
  ]

  test('ends an open session at its deadline', () => {
    const result = alarm(state(open(), onlyBrowser), T0 + Policy.openTtlMs)
    expect(result.record).toBeNull()
    expect(result.actions).toEqual(endedWith(CloseCode.expired))
  })

  test('ends a session at the hard cap even if it was renewed past it', () => {
    const result = alarm(
      state(paired({ expiresAt: T0 + Policy.hardTtlMs + MINUTE }), both),
      T0 + Policy.hardTtlMs,
    )
    expect(result.actions).toEqual(endedWith(CloseCode.expired))
  })

  test('ends a session whose visitor stayed away past the grace', () => {
    const record = paired({ browserGoneAt: T0 })
    const result = alarm(state(record, onlyAgent), T0 + Policy.browserGraceMs)
    expect(result.record).toBeNull()
    expect(result.actions).toEqual(endedWith(CloseCode.visitorGone))
  })

  test('reschedules while a renewal pushed the deadline out', () => {
    const record = paired({ expiresAt: T0 + 3 * Policy.pairedTtlMs })
    const result = alarm(state(record, both), T0 + Policy.pairedTtlMs)
    expect(result.record).toBe(record)
    expect(result.actions).toEqual([
      { _tag: 'SetAlarm', at: nextDeadline(record) },
    ])
  })

  test('waits out the grace period when the visitor is still away', () => {
    const record = paired({ browserGoneAt: T0 })
    const result = alarm(state(record, onlyAgent), T0 + MINUTE)
    expect(result.actions).toEqual([
      { _tag: 'SetAlarm', at: T0 + Policy.browserGraceMs },
    ])
  })

  test('running twice is a no-op the second time', () => {
    const first = alarm(state(open(), onlyBrowser), T0 + Policy.openTtlMs)
    const second = alarm(state(first.record, nobody), T0 + Policy.openTtlMs)
    expect(second).toEqual({ record: null, actions: [] })
  })
})

describe('classify', () => {
  const run1 = (...args: Parameters<typeof classify>) =>
    Effect.runSync(classify(...args))

  test('decodes a browser control message', () => {
    const text = JSON.stringify({ _tag: 'Bye' })
    expect(run1('browser', text)).toEqual({
      _tag: 'BrowserText',
      message: { _tag: 'Bye' },
      text,
    })
  })

  test('decodes an agent control message and keeps its text to forward', () => {
    const text = JSON.stringify({
      _tag: 'Op',
      id: 1,
      op: { _tag: 'Enter', lease: '0123456789abcdef' },
    })
    expect(run1('agent', text)).toMatchObject({ _tag: 'AgentText', text })
  })

  test('refuses a serial number smuggled into a device announcement', () => {
    const text = JSON.stringify({
      _tag: 'DeviceAnnounced',
      device: {
        _tag: 'serial',
        identity: { vendorId: 1, productId: 2, serialNumber: 'AA:BB' },
      },
    })
    expect(run1('browser', text)).toEqual({
      _tag: 'Malformed',
      from: 'browser',
    })
  })

  test('refuses a field the schema does not know on any message', () => {
    expect(
      run1('browser', JSON.stringify({ _tag: 'Bye', serialNumber: 'x' })),
    ).toEqual({ _tag: 'Malformed', from: 'browser' })
  })

  test.each([
    [
      'browser',
      { _tag: 'Op', id: 1, op: { _tag: 'Enter', lease: '0123456789abcdef' } },
    ],
    ['agent', { _tag: 'Bye' }],
    ['agent', { _tag: 'Welcome' }],
  ] as const)('refuses a wrong-direction message from the %s', (from, body) => {
    expect(run1(from, JSON.stringify(body))).toEqual({
      _tag: 'Malformed',
      from,
    })
  })

  test('refuses text that is not JSON', () => {
    expect(run1('agent', 'hello')).toEqual({ _tag: 'Malformed', from: 'agent' })
  })

  test('answers a stray pong with nothing to do', () => {
    expect(run1('browser', 'pong')).toEqual({ _tag: 'Pong' })
  })
})
