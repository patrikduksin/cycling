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
} from '@cycling/protocol'

import type { Link, RemoteStatus } from './RemotePort.ts'
import type { AdmissionError, Ending, SessionError } from './errors.ts'

export const TOKEN_TOO_SHORT =
  'CYCLING_REMOTE_TOKEN must be the AGENT_TOKEN the relay was deployed with (at least 32 characters)'

export const RELAY_NOT_TLS =
  'CYCLING_REMOTE_RELAY must be wss://<host> (ws:// only for localhost, 127.0.0.1 or [::1]), with no path'

export const describeConfig = (message: string): string =>
  `agent environment: ${message}`

export const WAITING_FOR_DEVICE = 'waiting for the visitor to choose a device'

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

export const describeError = (error: SessionError): string =>
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
  })

export const cannotRun = (command: string, detail: string): string =>
  `cannot run ${command}: ${detail}`

export const childStopped = (command: string, detail: string): string =>
  `${command} stopped without an exit code: ${detail}`

const describeLink = (link: Link): string =>
  Match.valueTags(link, {
    Connecting: () => 'connecting',
    Joined: ({ browserPresent }) => (browserPresent ? 'up' : 'visitor away'),
    Redialing: () => 'relay redialing',
    Over: ({ ending }) => describeEnding(ending),
  })

/** The part of the status line that changes rarely: printed as a new line when it changes and stdout is not a TTY. */
export const statusState = (status: RemoteStatus): string => {
  const port = Option.match(status.port, {
    onNone: () => 'no port yet',
    onSome: ({ epoch, state }) => `epoch ${epoch}  ${describePort(state)}`,
  })
  return `${describeLink(status.link)}  ${port}`
}

/** `dropped` counts bytes written while the relay or the port was down, and device bytes no pty reader took. */
export const statusLine = (options: {
  readonly status: RemoteStatus
  readonly ptyDropped: number
}): string => {
  const { status, ptyDropped } = options
  const { fromDevice, toDevice, dropped } = status.traffic
  const lost = dropped + ptyDropped
  const drops = lost > 0 ? `  dropped ${formatBytes(lost)}` : ''
  return `[ ${statusState(status)}  from device ${formatBytes(fromDevice)}  to device ${formatBytes(toDevice)}${drops} ]`
}

export const endpointBlock = (options: {
  readonly code: Code
  readonly relay: string
  readonly device: Device
  readonly pty: string
}): string =>
  [
    `joined ${formatCode(options.code)} via ${options.relay}`,
    `device   ${describeDevice(options.device)}`,
    `  CYCLING_PORT=${options.pty}`,
  ].join('\n')
