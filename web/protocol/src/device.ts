import * as Schema from 'effect/Schema'

import { UsbId } from './ids.ts'

/**
 * What Web Serial says about a port: two integers. No module here has a
 * serial-number field (on the C606 it is the MAC), and every decoder rejects
 * excess properties, so a client that adds one is refused, not stored.
 */
export const SerialIdentity = Schema.Struct({
  vendorId: UsbId,
  productId: UsbId,
})
export type SerialIdentity = typeof SerialIdentity.Type

const Text = Schema.String.check(Schema.isMaxLength(64))

/** WebUSB descriptor fields readable without `open()`; `serialNumber` is deliberately absent. */
export const UsbDescriptor = Schema.Struct({
  vendorId: UsbId,
  productId: UsbId,
  usbVersion: Text,
  deviceClass: Schema.Int,
  deviceSubclass: Schema.Int,
  deviceProtocol: Schema.Int,
  deviceVersion: Text,
  manufacturerName: Schema.optionalKey(Text),
  productName: Schema.optionalKey(Text),
})
export type UsbDescriptor = typeof UsbDescriptor.Type

/** One announced device per session: `serial` is bridged, `usb` is describe-only. */
export const Device = Schema.Union([
  Schema.TaggedStruct('serial', { identity: SerialIdentity }),
  Schema.TaggedStruct('usb', { descriptor: UsbDescriptor }),
])
export type Device = typeof Device.Type

const hex4 = (id: number) => id.toString(16).padStart(4, '0')

/** `303a:1001`, the form lsusb prints. */
export const formatUsbId = (ids: {
  readonly vendorId: number
  readonly productId: number
}): string => `${hex4(ids.vendorId)}:${hex4(ids.productId)}`
