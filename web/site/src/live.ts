import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import { modifyFields } from 'foldkit/struct'

import {
  type BrowserMsg,
  Epoch,
  type PortFailureReason,
  type PortState,
  type RequestId,
} from '@cycling/protocol'

import {
  DEFAULT_BAUD,
  type Port,
  type PortPhase,
  Source,
  type Session,
  type StoredSession,
} from './model.ts'

/** The port's next value and what the agent hears about it. */
export interface Step {
  readonly port: Port
  /** OpResults and PortChanged, in wire order. */
  readonly messages: ReadonlyArray<BrowserMsg>
  /** An Enter to answer from the held session, so the MAC skips `update`. */
  readonly entered: Option.Option<RequestId>
}

export const step = (
  port: Port,
  messages: ReadonlyArray<BrowserMsg> = [],
  entered: Option.Option<RequestId> = Option.none(),
): Step => ({ port, messages, entered })

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
  ops: [],
  activity: [],
})

/** A port recovered after a reload: the granted port is probably there already, but may still be resetting. */
export const recoveredPort = (stored: StoredSession): Port => ({
  identity: stored.identity,
  baud: stored.baud,
  attempt: 1,
  epoch: stored.epoch,
  state: REACQUIRING,
  ops: [],
  activity: [],
})

export const storedOf = (
  session: Session,
  port: Port,
  writesAllowed: boolean,
): StoredSession => ({
  session,
  identity: port.identity,
  baud: port.baud,
  epoch: port.epoch,
  writesAllowed,
})

const portChangedTo = (port: Port, state: PortState): BrowserMsg => ({
  _tag: 'PortChanged',
  status: { epoch: port.epoch, state },
})

/** A hold is `Flashing` on the wire from the moment the console lets go. */
const wireState = (phase: PortPhase): PortState =>
  phase._tag === 'Entering' || phase._tag === 'Flashing'
    ? { _tag: 'Flashing' }
    : phase

export const portChanged = (port: Port): BrowserMsg =>
  portChangedTo(port, wireState(port.state))

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
export const reacquire = (port: Port): Step =>
  step(
    modifyFields(port, {
      attempt: attempt => attempt + 1,
      state: () => REACQUIRING,
    }),
    [portChangedTo(port, LOST), portChangedTo(port, REACQUIRING)],
  )

export const opened = (port: Port): Step => {
  const next = modifyFields(port, {
    epoch: nextEpoch,
    state: () => ({ _tag: 'Open', baud: port.baud }),
  })
  return step(next, [portChanged(next)])
}

export const failed = (port: Port, reason: PortFailureReason): Step => {
  const next = modifyFields(port, {
    state: () => ({ _tag: 'Failed', reason }),
  })
  return step(next, [portChanged(next)])
}

/** Reopening the port in place of a chooser click: the chosen port replaces the failed one. */
export const replaced = (port: Port, identity: Port['identity']): Step => {
  const next = modifyFields(port, {
    identity: () => identity,
    attempt: attempt => attempt + 1,
    state: () => OPENING,
  })
  return step(next, [
    { _tag: 'DeviceAnnounced', device: { _tag: 'serial', identity } },
    portChanged(next),
  ])
}
