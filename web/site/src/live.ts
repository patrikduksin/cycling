import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import { modifyFields } from 'foldkit/struct'

import {
  type BrowserMsg,
  Epoch,
  type PortFailureReason,
  type PortState,
  type RequestId,
  type RequestOutcome,
} from '@cycling/protocol'

import {
  DEFAULT_BAUD,
  type Port,
  type PortPhase,
  Source,
  type Session,
  type StoredSession,
} from './model.ts'

/** The port's next value and what the agent hears about it, in order. */
export interface PortStep {
  readonly port: Port
  readonly messages: ReadonlyArray<BrowserMsg>
}

const decodeEpoch = Schema.decodeSync(Epoch)

export const firstEpoch = decodeEpoch(0)
export const nextEpoch = (epoch: Epoch): Epoch => decodeEpoch(epoch + 1)

const OPENING: PortPhase = { _tag: 'Opening' }
const LOST: PortState = { _tag: 'Lost' }
const REACQUIRING: PortPhase = { _tag: 'Reacquiring' }

export const newPort = (identity: Port['identity']): Port => ({
  identity,
  baud: DEFAULT_BAUD,
  attempt: 0,
  epoch: firstEpoch,
  state: OPENING,
  pendingLineCoding: Option.none(),
})

/** A port recovered after a reload: the granted port is probably there already, but may still be resetting. */
export const recoveredPort = (stored: StoredSession): Port => ({
  identity: stored.identity,
  baud: stored.baud,
  attempt: 1,
  epoch: stored.epoch,
  state: REACQUIRING,
  pendingLineCoding: Option.none(),
})

export const storedOf = (session: Session, port: Port): StoredSession => ({
  session,
  identity: port.identity,
  baud: port.baud,
  epoch: port.epoch,
})

const portChangedTo = (port: Port, state: PortState): BrowserMsg => ({
  _tag: 'PortChanged',
  status: { epoch: port.epoch, state },
})

export const portChanged = (port: Port): BrowserMsg =>
  portChangedTo(port, port.state)

export const announcement = (source: Source): ReadonlyArray<BrowserMsg> =>
  Source.match<ReadonlyArray<BrowserMsg>>(source, {
    Serial: ({ port }) => [
      {
        _tag: 'DeviceAnnounced',
        device: { _tag: 'serial', identity: port.identity },
      },
      portChanged(port),
    ],
    Usb: ({ descriptor }) => [
      { _tag: 'DeviceAnnounced', device: { _tag: 'usb', descriptor } },
    ],
  })

/**
 * Every unplug walks Lost then Reacquiring on the wire. The Model skips Lost
 * and rests in Reacquiring, and bumping `attempt` makes the runtime close the
 * dead port and wait for the granted one to return.
 */
export const reacquire = (
  port: Port,
  options: {
    readonly baud: number
    readonly lineCodingId: Port['pendingLineCoding']
  },
): PortStep => ({
  port: modifyFields(port, {
    baud: () => options.baud,
    attempt: attempt => attempt + 1,
    state: () => REACQUIRING,
    pendingLineCoding: () => options.lineCodingId,
  }),
  messages: [portChangedTo(port, LOST), portChangedTo(port, REACQUIRING)],
})

export const opened = (port: Port): PortStep => {
  const next = modifyFields(port, {
    epoch: nextEpoch,
    state: () => ({ _tag: 'Open', baud: port.baud }),
    pendingLineCoding: () => Option.none(),
  })
  return {
    port: next,
    messages: [
      portChanged(next),
      ...Option.match(port.pendingLineCoding, {
        onNone: () => [],
        onSome: id => [lineCodingResult(id, 'done')],
      }),
    ],
  }
}

export const failed = (port: Port, reason: PortFailureReason): PortStep => {
  const next = modifyFields(port, {
    state: () => ({ _tag: 'Failed', reason }),
    pendingLineCoding: () => Option.none(),
  })
  return {
    port: next,
    messages: [
      portChanged(next),
      ...Option.match(port.pendingLineCoding, {
        onNone: () => [],
        onSome: id => [lineCodingResult(id, 'failed')],
      }),
    ],
  }
}

const lineCodingResult = (
  id: RequestId,
  outcome: RequestOutcome,
): BrowserMsg => ({ _tag: 'LineCodingResult', id, outcome })

/** Reopening the port in place of a chooser click: the chosen port replaces the failed one. */
export const replaced = (port: Port, identity: Port['identity']): PortStep => {
  const next = modifyFields(port, {
    identity: () => identity,
    attempt: attempt => attempt + 1,
    state: () => OPENING,
    pendingLineCoding: () => Option.none(),
  })
  return {
    port: next,
    messages: [
      {
        _tag: 'DeviceAnnounced',
        device: { _tag: 'serial', identity },
      },
      portChanged(next),
    ],
  }
}
