import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import {
  Baud,
  Code,
  Epoch,
  OwnerKey,
  RequestId,
  SerialIdentity,
  UsbDescriptor,
} from '@cycling/protocol'

import {
  DEFAULT_BAUD,
  type Model,
  Page,
  type PortPhase,
  type QueuedOp,
  type Relay,
  Relay as RelayStates,
  type Session,
  Source,
} from '../model.ts'
import { emptyMonitor } from '../monitor.ts'

export const identity = Schema.decodeSync(SerialIdentity)({
  vendorId: 0x303a,
  productId: 0x1001,
})

export const otherIdentity = Schema.decodeSync(SerialIdentity)({
  vendorId: 0x10c4,
  productId: 0xea60,
})

export const session: Session = {
  code: Schema.decodeSync(Code)('7KQM4XP2'),
  ownerKey: Schema.decodeSync(OwnerKey)('AAAAAAAAAAAAAAAAAAAAAA'),
}

export const nextSession: Session = {
  code: Schema.decodeSync(Code)('0123456Z'),
  ownerKey: Schema.decodeSync(OwnerKey)('BBBBBBBBBBBBBBBBBBBBBB'),
}

export const descriptor = Schema.decodeSync(UsbDescriptor)({
  vendorId: 0x22d9,
  productId: 0x2765,
  usbVersion: '2.0.0',
  deviceClass: 6,
  deviceSubclass: 1,
  deviceProtocol: 1,
  deviceVersion: '4.1.0',
})

export const epoch = Schema.decodeSync(Epoch)
export const requestId = Schema.decodeSync(RequestId)
export const baud = Schema.decodeSync(Baud)

export const idle: Model = {
  canDescribeUsb: true,
  page: Page.Idle({ notice: Option.none() }),
}

export const serialLive = (
  patch: {
    readonly state?: PortPhase
    readonly attempt?: number
    readonly epoch?: number
    readonly baud?: number
    readonly relay?: Relay
    readonly ops?: ReadonlyArray<QueuedOp>
    readonly writesAllowed?: boolean
  } = {},
): Model => ({
  canDescribeUsb: true,
  page: Page.Live({
    session,
    source: Source.Serial({
      port: {
        identity,
        baud: baud(patch.baud ?? DEFAULT_BAUD),
        attempt: patch.attempt ?? 0,
        epoch: epoch(patch.epoch ?? 1),
        state: patch.state ?? {
          _tag: 'Open',
          baud: patch.baud ?? DEFAULT_BAUD,
        },
        ops: patch.ops ?? [],
        activity: [],
      },
    }),
    relay:
      patch.relay ?? RelayStates.Joined({ attempt: 0, agentPresent: true }),
    monitor: emptyMonitor,
    copied: false,
    writesAllowed: patch.writesAllowed ?? false,
    confirmingStop: false,
  }),
})

export const usbLive = (relay?: Relay): Model => ({
  canDescribeUsb: true,
  page: Page.Live({
    session,
    source: Source.Usb({ descriptor }),
    relay: relay ?? RelayStates.Joined({ attempt: 0, agentPresent: false }),
    monitor: emptyMonitor,
    copied: false,
    writesAllowed: false,
    confirmingStop: false,
  }),
})
