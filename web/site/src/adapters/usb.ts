import * as Effect from 'effect/Effect'

import type { UsbDescriptor } from '@cycling/protocol'

import { ChooserCancelled, ChooserFailed, type UsbAdapter } from '../links.ts'

/**
 * The fields of a WebUSB device this page may read. `serialNumber` is left
 * out on purpose, so no code path through `describeUsbDevice` can reach it
 * (on the C606 it is the MAC address).
 */
export interface UsbDeviceFields {
  readonly vendorId: number
  readonly productId: number
  readonly usbVersionMajor: number
  readonly usbVersionMinor: number
  readonly usbVersionSubminor: number
  readonly deviceClass: number
  readonly deviceSubclass: number
  readonly deviceProtocol: number
  readonly deviceVersionMajor: number
  readonly deviceVersionMinor: number
  readonly deviceVersionSubminor: number
  readonly manufacturerName?: string | null | undefined
  readonly productName?: string | null | undefined
}

const MAX_TEXT = 64

const version = (major: number, minor: number, subminor: number): string =>
  `${major}.${minor}.${subminor}`

export const describeUsbDevice = (device: UsbDeviceFields): UsbDescriptor => ({
  vendorId: device.vendorId,
  productId: device.productId,
  usbVersion: version(
    device.usbVersionMajor,
    device.usbVersionMinor,
    device.usbVersionSubminor,
  ),
  deviceClass: device.deviceClass,
  deviceSubclass: device.deviceSubclass,
  deviceProtocol: device.deviceProtocol,
  deviceVersion: version(
    device.deviceVersionMajor,
    device.deviceVersionMinor,
    device.deviceVersionSubminor,
  ),
  ...(typeof device.manufacturerName === 'string'
    ? { manufacturerName: device.manufacturerName.slice(0, MAX_TEXT) }
    : {}),
  ...(typeof device.productName === 'string'
    ? { productName: device.productName.slice(0, MAX_TEXT) }
    : {}),
})

const CLASS_NAMES: Readonly<Record<number, string>> = {
  0x00: 'Per interface',
  0x01: 'Audio',
  0x02: 'Communications',
  0x03: 'HID',
  0x05: 'Physical',
  0x06: 'Still Image',
  0x07: 'Printer',
  0x08: 'Mass Storage',
  0x09: 'Hub',
  0x0a: 'CDC Data',
  0x0b: 'Smart Card',
  0x0d: 'Content Security',
  0x0e: 'Video',
  0x0f: 'Personal Healthcare',
  0x10: 'Audio/Video',
  0xdc: 'Diagnostic',
  0xe0: 'Wireless Controller',
  0xef: 'Miscellaneous',
  0xfe: 'Application Specific',
  0xff: 'Vendor Specific',
}

export const usbClassName = (deviceClass: number): string =>
  CLASS_NAMES[deviceClass] ?? `Class ${deviceClass}`

const request: UsbAdapter['request'] = Effect.tryPromise({
  try: () => navigator.usb.requestDevice({ filters: [] }),
  catch: error =>
    error instanceof DOMException && error.name === 'NotFoundError'
      ? new ChooserCancelled()
      : new ChooserFailed({
          reason: error instanceof Error ? error.message : 'Unknown error',
        }),
}).pipe(Effect.map(describeUsbDevice))

export const webUsb: UsbAdapter = { request }
