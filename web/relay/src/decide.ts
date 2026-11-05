import * as Match from 'effect/Match'

import {
  type AgentMsg,
  type BrowserMsg,
  CloseCode,
  type EndReason,
  type RelayMsg,
  type Role,
  type Snapshot,
} from '@cycling/protocol'

import { Policy } from './Policy.ts'
import type { SessionRecord } from './SessionRecord.ts'

/** Which live sockets an action reaches. `Others` never includes the socket the event came from. */
export type Audience =
  | { readonly _tag: 'Subject' }
  | { readonly _tag: 'Others'; readonly role: Role }
  | { readonly _tag: 'Everyone' }

export type Action =
  | {
      readonly _tag: 'Send'
      readonly to: Audience
      readonly message: RelayMsg
    }
  | { readonly _tag: 'Forward'; readonly to: Role; readonly text: string }
  | {
      readonly _tag: 'Close'
      readonly who: Audience
      readonly code: CloseCode
    }
  | { readonly _tag: 'SetAlarm'; readonly at: number }
  | { readonly _tag: 'ClearAlarm' }

/** One thing that happened to the session. The socket it happened on is the subject. */
export type Event =
  | { readonly _tag: 'BrowserUpgrade'; readonly ownerKeyHash: string }
  | { readonly _tag: 'AgentUpgrade' }
  | {
      readonly _tag: 'BrowserText'
      readonly message: BrowserMsg
      readonly text: string
    }
  | {
      readonly _tag: 'AgentText'
      readonly message: AgentMsg
      readonly text: string
    }
  | { readonly _tag: 'Malformed'; readonly from: Role }
  | { readonly _tag: 'Pong' }
  | { readonly _tag: 'Closed'; readonly role: Role }
  | { readonly _tag: 'Alarm' }

/** Live sockets per role, not counting the event's subject. */
export type Presence = Readonly<Record<Role, boolean>>

export interface State {
  readonly record: SessionRecord | null
  readonly presence: Presence
}

/** The record to store (the same reference when nothing changed) and what to do about it. */
export interface Decision {
  readonly record: SessionRecord | null
  readonly actions: ReadonlyArray<Action>
}

const subject: Audience = { _tag: 'Subject' }
const everyone: Audience = { _tag: 'Everyone' }
const others = (role: Role): Audience => ({ _tag: 'Others', role })

export const otherRole = (role: Role): Role =>
  role === 'browser' ? 'agent' : 'browser'

const send = (to: Audience, message: RelayMsg): Action => ({
  _tag: 'Send',
  to,
  message,
})
const close = (who: Audience, code: CloseCode): Action => ({
  _tag: 'Close',
  who,
  code,
})
const forward = (to: Role, text: string): Action => ({
  _tag: 'Forward',
  to,
  text,
})

const hardDeadline = (record: SessionRecord): number =>
  record.createdAt + Policy.hardTtlMs

/** The earliest moment the alarm has anything to decide. */
export const nextDeadline = (record: SessionRecord): number => {
  const expiry = Math.min(record.expiresAt, hardDeadline(record))
  return record.browserGoneAt === null
    ? expiry
    : Math.min(expiry, record.browserGoneAt + Policy.browserGraceMs)
}

const slidingExpiry = (record: SessionRecord, now: number): number =>
  Math.min(now + Policy.pairedTtlMs, hardDeadline(record))

/** A text frame keeps a paired session alive; an open one keeps its fixed deadline. */
const touch = (record: SessionRecord, now: number): SessionRecord =>
  record._tag === 'Paired'
    ? { ...record, expiresAt: slidingExpiry(record, now) }
    : record

const snapshot = (record: SessionRecord, present: Presence): Snapshot => ({
  device: record.device,
  port: record.port,
  expiresAt: record.expiresAt,
  browserPresent: present.browser,
  agentPresent: present.agent,
})

const welcome = (
  role: Role,
  record: SessionRecord,
  presence: Presence,
): Action =>
  send(subject, {
    _tag: 'Welcome',
    role,
    snapshot: snapshot(record, { ...presence, [role]: true }),
  })

const end = (reason: EndReason): Decision => ({
  record: null,
  actions: [close(everyone, CloseCode[reason]), { _tag: 'ClearAlarm' }],
})

const arrive = (
  role: Role,
  record: SessionRecord,
  presence: Presence,
): Decision => ({
  record,
  actions: [
    ...(presence[role] ? [close(others(role), CloseCode.replaced)] : []),
    welcome(role, record, presence),
    send(others(otherRole(role)), {
      _tag: 'PeerPresence',
      role,
      present: true,
    }),
    { _tag: 'SetAlarm', at: nextDeadline(record) },
  ],
})

const browserUpgrade = (
  state: State,
  ownerKeyHash: string,
  now: number,
): Decision => {
  const { record, presence } = state
  if (record === null) {
    const created: SessionRecord = {
      _tag: 'Open',
      ownerKeyHash,
      createdAt: now,
      expiresAt: now + Policy.openTtlMs,
      browserGoneAt: null,
      device: null,
      port: null,
    }
    return arrive('browser', created, presence)
  }
  if (record.ownerKeyHash !== ownerKeyHash) {
    return { record, actions: [close(subject, CloseCode.codeInUse)] }
  }
  return arrive('browser', { ...record, browserGoneAt: null }, presence)
}

const agentUpgrade = (state: State, now: number): Decision => {
  const { record, presence } = state
  if (record === null) {
    return { record, actions: [close(subject, CloseCode.notFound)] }
  }
  const paired: SessionRecord = {
    ...record,
    _tag: 'Paired',
    expiresAt: slidingExpiry(record, now),
  }
  return arrive('agent', paired, presence)
}

/** A control message the relay passes on verbatim, after storing what it carries. */
const control = (
  record: SessionRecord,
  patch: Partial<Pick<SessionRecord, 'device' | 'port'>>,
  from: Role,
  text: string,
  now: number,
): Decision => ({
  record: touch({ ...record, ...patch }, now),
  actions: [forward(otherRole(from), text)],
})

const browserText = (
  record: SessionRecord,
  message: BrowserMsg,
  text: string,
  now: number,
): Decision => {
  const pass = (patch: Partial<Pick<SessionRecord, 'device' | 'port'>> = {}) =>
    control(record, patch, 'browser', text, now)
  return Match.valueTags(message, {
    DeviceAnnounced: ({ device }) => pass({ device }),
    PortChanged: ({ status }) => pass({ port: status }),
    OpProgress: () => pass(),
    OpResult: () => pass(),
    Bye: () => end('visitorDisconnected'),
  })
}

const closed = (state: State, role: Role, now: number): Decision => {
  const { record, presence } = state
  if (record === null || presence[role]) {
    return { record, actions: [] }
  }
  const notify = send(others(otherRole(role)), {
    _tag: 'PeerPresence',
    role,
    present: false,
  })
  if (role === 'agent') {
    return { record, actions: [notify] }
  }
  const gone: SessionRecord = { ...record, browserGoneAt: now }
  return {
    record: gone,
    actions: [notify, { _tag: 'SetAlarm', at: nextDeadline(gone) }],
  }
}

const alarm = (state: State, now: number): Decision => {
  const { record, presence } = state
  if (record === null) {
    return { record, actions: [] }
  }
  if (now >= Math.min(record.expiresAt, hardDeadline(record))) {
    return end('expired')
  }
  if (
    record.browserGoneAt !== null &&
    !presence.browser &&
    now >= record.browserGoneAt + Policy.browserGraceMs
  ) {
    return end('visitorGone')
  }
  return { record, actions: [{ _tag: 'SetAlarm', at: nextDeadline(record) }] }
}

const violation: ReadonlyArray<Action> = [
  close(subject, CloseCode.protocolViolation),
]

/** A text frame on a record that is gone: the code is unknown again. */
const sessionOver: Decision = {
  record: null,
  actions: [close(subject, CloseCode.notFound)],
}

/**
 * The whole relay policy as a table: what each event does to the record and
 * which socket actions follow. The Durable Object only interprets the result.
 */
export const decide = (state: State, event: Event, now: number): Decision => {
  const { record } = state
  return Match.valueTags(event, {
    BrowserUpgrade: ({ ownerKeyHash }) =>
      browserUpgrade(state, ownerKeyHash, now),
    AgentUpgrade: () => agentUpgrade(state, now),
    BrowserText: ({ message, text }) =>
      record === null ? sessionOver : browserText(record, message, text, now),
    AgentText: ({ text }) =>
      record === null ? sessionOver : control(record, {}, 'agent', text, now),
    Malformed: () => ({ record, actions: violation }),
    Pong: () => ({ record, actions: [] }),
    Closed: ({ role }) => closed(state, role, now),
    Alarm: () => alarm(state, now),
  })
}
