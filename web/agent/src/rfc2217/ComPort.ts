import * as Match from 'effect/Match'

import type { LineCommand } from '../ResetRecognizer.ts'
import { type Item, negotiation, subnegotiation, type Verb } from './Telnet.ts'

const BINARY = 0
const SGA = 3
const COM_PORT_OPTION = 44
const SUPPORTED: ReadonlySet<number> = new Set([BINARY, SGA, COM_PORT_OPTION])

const SERVER = 100
const SET_BAUDRATE = 1
const SET_DATASIZE = 2
const SET_PARITY = 3
const SET_STOPSIZE = 4
const SET_CONTROL = 5
const NOTIFY_LINESTATE = 6
const NOTIFY_MODEMSTATE = 7
const SET_LINESTATE_MASK = 10
const SET_MODEMSTATE_MASK = 11
const PURGE_DATA = 12

const CONTROL = {
  reqFlow: 0,
  noFlow: 1,
  reqBreak: 4,
  breakOff: 6,
  reqDtr: 7,
  dtrOn: 8,
  dtrOff: 9,
  reqRts: 10,
  rtsOn: 11,
  rtsOff: 12,
  reqFlowIn: 13,
  noFlowIn: 14,
} as const

const LINE_COMMANDS: ReadonlyMap<number, LineCommand> = new Map([
  [CONTROL.dtrOn, { line: 'dtr', on: true }],
  [CONTROL.dtrOff, { line: 'dtr', on: false }],
  [CONTROL.rtsOn, { line: 'rts', on: true }],
  [CONTROL.rtsOff, { line: 'rts', on: false }],
])

/** What a client asked of the port, after the telnet layer answered it locally. */
export type ClientRequest =
  | { readonly _tag: 'Data'; readonly bytes: Uint8Array }
  | { readonly _tag: 'Line'; readonly command: LineCommand }
  | { readonly _tag: 'Purge' }
  | { readonly _tag: 'SetBaud'; readonly baud: number }

export interface Session {
  /** Options this server has agreed to perform (WILL) and asked the client to perform (DO). */
  readonly will: ReadonlySet<number>
  readonly do: ReadonlySet<number>
  readonly dtr: boolean
  readonly rts: boolean
  readonly baud: number
}

export interface Step {
  readonly session: Session
  readonly reply: ReadonlyArray<Uint8Array>
  readonly request: ClientRequest | null
}

const comPort = (command: number, value: ReadonlyArray<number>) =>
  subnegotiation(Uint8Array.of(COM_PORT_OPTION, SERVER + command, ...value))

/**
 * Sent on accept. pyserial's client waits until we answer its WILL
 * COM-PORT-OPTION with DO; offering BINARY both ways keeps 0x0d and 0x00 raw.
 */
export const greet = (baud: number): Step => ({
  session: {
    will: new Set([BINARY, SGA, COM_PORT_OPTION]),
    do: new Set([BINARY, COM_PORT_OPTION]),
    dtr: false,
    rts: false,
    baud,
  },
  reply: [
    negotiation('WILL', COM_PORT_OPTION),
    negotiation('DO', COM_PORT_OPTION),
    negotiation('WILL', BINARY),
    negotiation('DO', BINARY),
    negotiation('WILL', SGA),
    comPort(NOTIFY_MODEMSTATE, [0]),
  ],
  request: null,
})

const without = (set: ReadonlySet<number>, option: number) =>
  new Set([...set].filter(entry => entry !== option))

/** Loop-free: answer only a request that changes what we agreed to. */
const negotiate = (session: Session, verb: Verb, option: number): Step => {
  const idle = { session, reply: [], request: null }
  switch (verb) {
    case 'DO':
      if (!SUPPORTED.has(option)) {
        return { ...idle, reply: [negotiation('WONT', option)] }
      }
      return session.will.has(option)
        ? idle
        : {
            ...idle,
            session: { ...session, will: new Set([...session.will, option]) },
            reply: [negotiation('WILL', option)],
          }
    case 'WILL':
      if (!SUPPORTED.has(option)) {
        return { ...idle, reply: [negotiation('DONT', option)] }
      }
      return session.do.has(option)
        ? idle
        : {
            ...idle,
            session: { ...session, do: new Set([...session.do, option]) },
            reply: [negotiation('DO', option)],
          }
    case 'DONT':
      return session.will.has(option)
        ? {
            ...idle,
            session: { ...session, will: without(session.will, option) },
            reply: [negotiation('WONT', option)],
          }
        : idle
    case 'WONT':
      return session.do.has(option)
        ? {
            ...idle,
            session: { ...session, do: without(session.do, option) },
            reply: [negotiation('DONT', option)],
          }
        : idle
  }
}

const be32 = (n: number) => [
  (n >>> 24) & 0xff,
  (n >>> 16) & 0xff,
  (n >>> 8) & 0xff,
  n & 0xff,
]

const control = (session: Session, value: number): Step => {
  const answer = (reply: number, request: ClientRequest | null = null) => ({
    session,
    reply: [comPort(SET_CONTROL, [reply])],
    request,
  })
  const line = LINE_COMMANDS.get(value)
  if (line !== undefined) {
    return {
      session: { ...session, [line.line]: line.on },
      reply: [comPort(SET_CONTROL, [value])],
      request: { _tag: 'Line', command: line },
    }
  }
  switch (value) {
    case CONTROL.reqFlow:
      return answer(CONTROL.noFlow)
    case CONTROL.reqBreak:
      return answer(CONTROL.breakOff)
    case CONTROL.reqDtr:
      return answer(session.dtr ? CONTROL.dtrOn : CONTROL.dtrOff)
    case CONTROL.reqRts:
      return answer(session.rts ? CONTROL.rtsOn : CONTROL.rtsOff)
    case CONTROL.reqFlowIn:
      return answer(CONTROL.noFlowIn)
    default:
      return answer(value)
  }
}

/**
 * Answers every COM-PORT-OPTION locally and at once, so pyserial's
 * stop-and-wait handshake costs no relay round trips. Only data, line
 * commands, purges and baud changes surface as requests; the caller decides
 * what reaches the device.
 */
const comPortRequest = (session: Session, payload: Uint8Array): Step => {
  const idle = { session, reply: [], request: null }
  const command = payload[1]
  const value = [...payload.subarray(2)]
  const first = value[0] ?? 0
  switch (command) {
    case SET_BAUDRATE: {
      const [a = 0, b = 0, c = 0, d = 0] = value
      const requested = ((a << 24) | (b << 16) | (c << 8) | d) >>> 0
      if (requested === 0) {
        return { ...idle, reply: [comPort(SET_BAUDRATE, be32(session.baud))] }
      }
      return {
        session: { ...session, baud: requested },
        reply: [comPort(SET_BAUDRATE, be32(requested))],
        request: { _tag: 'SetBaud', baud: requested },
      }
    }
    case SET_DATASIZE:
      return { ...idle, reply: [comPort(SET_DATASIZE, [first || 8])] }
    case SET_PARITY:
    case SET_STOPSIZE:
      return { ...idle, reply: [comPort(command, [first || 1])] }
    case SET_CONTROL:
      return control(session, first)
    case NOTIFY_LINESTATE:
    case NOTIFY_MODEMSTATE:
      return { ...idle, reply: [comPort(command, [0])] }
    case SET_LINESTATE_MASK:
    case SET_MODEMSTATE_MASK:
      return { ...idle, reply: [comPort(command, [first])] }
    case PURGE_DATA:
      return {
        ...idle,
        reply: [comPort(PURGE_DATA, [first])],
        request: { _tag: 'Purge' },
      }
    default:
      return idle
  }
}

export const receive = (session: Session, item: Item): Step =>
  Match.valueTags(item, {
    Data: ({ bytes }): Step => ({
      session,
      reply: [],
      request: { _tag: 'Data', bytes },
    }),
    Negotiate: ({ verb, option }) => negotiate(session, verb, option),
    Subnegotiation: ({ payload }): Step =>
      payload[0] === COM_PORT_OPTION
        ? comPortRequest(session, payload)
        : { session, reply: [], request: null },
    Command: (): Step => ({ session, reply: [], request: null }),
  })
