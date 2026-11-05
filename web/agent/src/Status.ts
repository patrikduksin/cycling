import * as Match from 'effect/Match'
import * as Option from 'effect/Option'

import {
  type Code,
  closeName,
  type Device,
  type EndReason,
  formatBytes,
  formatCode,
  formatUsbId,
  type PortState,
  ResetKind,
} from '@cycling/protocol'

import type { Doors } from './Doors.ts'
import type { Link, RemoteStatus } from './RemotePort.ts'
import type {
  AdmissionError,
  ControlRefused,
  Ending,
  RequestError,
  SessionError,
} from './errors.ts'
import type { Rfc2217Event } from './rfc2217/Rfc2217Door.ts'

export const TOKEN_TOO_SHORT =
  'CYCLING_REMOTE_TOKEN must be the AGENT_TOKEN the relay was deployed with (at least 32 characters)'

export const RELAY_NOT_TLS =
  'CYCLING_REMOTE_RELAY must be wss://<host> (ws:// only for localhost, 127.0.0.1 or [::1]), with no path'

export const describeConfig = (message: string): string =>
  `agent environment: ${message}`

export const WAITING_FOR_DEVICE = 'waiting for the visitor to choose a device'

export const controlUsage = `expected "reset <${ResetKind.literals.join('|')}>"`

export const describeDevice = (device: Device): string =>
  Match.valueTags(device, {
    serial: ({ identity }) => `usb-serial ${formatUsbId(identity)}`,
    usb: ({ descriptor }) => `usb ${formatUsbId(descriptor)} (describe only)`,
  })

const describePort = (state: PortState): string =>
  Match.valueTags(state, {
    Opening: () => 'port opening',
    Open: ({ baud }) => `port open ${baud}`,
    Lost: () => 'port lost',
    Reacquiring: () => 'port reconnecting',
    Failed: ({ reason }) =>
      reason === 'openRefused'
        ? 'port failed: the browser could not open it'
        : 'port failed: the device did not come back',
  })

const END_REASONS: Readonly<Record<EndReason, string>> = {
  visitorDisconnected: 'the visitor disconnected',
  visitorGone: 'the visitor left',
  expired: 'the session expired',
}

const describeClose = (code: number): string =>
  Option.match(closeName(code), {
    onNone: () => `${code}`,
    onSome: name => `${name}, ${code}`,
  })

export const describeAdmission = (error: AdmissionError): string =>
  Match.valueTags(error, {
    Unauthorized: () => 'the relay refused this agent token',
    SessionNotFound: () =>
      'no session with that code (wrong, ended or expired)',
    UnsupportedVersion: ({ why }) =>
      why === 'unsupportedVersion'
        ? 'the relay speaks another protocol version'
        : 'the relay refused a frame this agent sent (another protocol version?)',
    RelayUnreachable: ({ why }) =>
      Match.valueTags(why, {
        Dial: ({ detail }) => `relay unreachable: ${detail}`,
        Closed: ({ code }) =>
          `relay unreachable: it closed the connection (${describeClose(code)})`,
      }),
  })

export const describeEnding = (ending: Ending): string =>
  Match.valueTags(ending, {
    Ended: ({ reason }) => `session ended: ${END_REASONS[reason]}`,
    Replaced: () => 'another agent joined this session; this one stopped',
    Rejected: ({ error }) => describeAdmission(error),
  })

export const describeRequestError = (error: RequestError): string =>
  Match.valueTags(error, {
    PortNotOpen: () => 'the browser port is not open',
    RequestFailed: () => 'the browser could not perform it',
    RequestTimedOut: ({ ms }) => `no answer from the browser within ${ms} ms`,
    RelayDown: () => 'the relay is reconnecting',
  })

export const describeError = (error: SessionError | ControlRefused): string =>
  Match.valueTags(error, {
    Unauthorized: describeAdmission,
    SessionNotFound: describeAdmission,
    UnsupportedVersion: describeAdmission,
    RelayUnreachable: describeAdmission,
    BadCode: ({ input }) =>
      `"${input}" is not a session code (8 characters, like 7KQM-4XP2)`,
    LocalResourceError: ({ resource, detail }) =>
      `could not set up the ${resource}: ${detail}`,
    PortNeverOpened: ({ seconds, state }) =>
      `the visitor's port did not open within ${seconds} s (${state})`,
    ControlRefused: ({ kind, why }) =>
      `reset ${kind} failed: ${Match.valueTags(why, {
        NoAgent: ({ code }) => `no agent is running for ${formatCode(code)}`,
        Answered: ({ reply }) => (reply === '' ? 'no answer' : reply),
      })}`,
  })

export const resetDone = (kind: ResetKind): string => `reset ${kind}: ok`

export const cannotRun = (command: string, detail: string): string =>
  `cannot run ${command}: ${detail}`

export const childStopped = (command: string, detail: string): string =>
  `${command} stopped without an exit code: ${detail}`

export const describeNotice = (event: Rfc2217Event): Option.Option<string> => {
  if (event._tag === 'Refused') {
    return Option.some('rfc2217: refused a second client (one at a time)')
  }
  if (event._tag !== 'Reset') return Option.none()
  return Option.some(
    Option.match(event.result, {
      onNone: () => `rfc2217: ${event.kind} done`,
      onSome: error =>
        `rfc2217: ${event.kind} failed, ${describeRequestError(error)}`,
    }),
  )
}

const describeLink = (link: Link): string =>
  Match.valueTags(link, {
    Connecting: () => 'connecting',
    Joined: ({ browserPresent }) => (browserPresent ? 'up' : 'visitor away'),
    Redialing: () => 'relay redialing',
    Over: ({ ending }) => describeEnding(ending),
  })

/** The part of the status line that changes rarely: printed as a new line when it changes and stdout is not a TTY. */
export const statusState = (status: RemoteStatus, doors: Doors): string => {
  const port = Option.match(status.port, {
    onNone: () => 'no port yet',
    onSome: ({ epoch, state }) => `epoch ${epoch}  ${describePort(state)}`,
  })
  return `${describeLink(status.link)}  ${port}  door ${doors.active}`
}

/** `dropped` counts bytes written while the relay or the port was down, strays from the inactive door, and device bytes no pty reader took. */
export const statusLine = (options: {
  readonly status: RemoteStatus
  readonly doors: Doors
  readonly ptyDropped: number
}): string => {
  const { status, doors, ptyDropped } = options
  const { fromDevice, toDevice, dropped } = status.traffic
  const lost = doors.dropped.pty + doors.dropped.rfc2217 + dropped + ptyDropped
  const drops = lost > 0 ? `  dropped ${formatBytes(lost)}` : ''
  return `[ ${statusState(status, doors)}  from device ${formatBytes(fromDevice)}  to device ${formatBytes(toDevice)}${drops} ]`
}

export const endpointBlock = (options: {
  readonly code: Code
  readonly relay: string
  readonly device: Device
  readonly pty: string
  readonly rfc2217: string
}): string =>
  [
    `joined ${formatCode(options.code)} via ${options.relay}`,
    `device   ${describeDevice(options.device)}`,
    `  CYCLING_PORT=${options.pty}`,
    `  CYCLING_ESPTOOL_PORT=${options.rfc2217}`,
  ].join('\n')
