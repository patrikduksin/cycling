import { describe, expect, test } from 'bun:test'
import * as Schema from 'effect/Schema'

import { UsbDescriptor, formatUsbId } from '@cycling/protocol'

import { describeUsbDevice, usbClassName } from './usb.ts'

const phone = {
  vendorId: 0x22d9,
  productId: 0x2765,
  usbVersionMajor: 2,
  usbVersionMinor: 0,
  usbVersionSubminor: 0,
  deviceClass: 6,
  deviceSubclass: 1,
  deviceProtocol: 1,
  deviceVersionMajor: 4,
  deviceVersionMinor: 1,
  deviceVersionSubminor: 0,
  manufacturerName: 'Acme',
  productName: 'Phone',
}

describe('describeUsbDevice', () => {
  test('maps the readable descriptor fields', () => {
    expect(describeUsbDevice(phone)).toEqual({
      vendorId: 0x22d9,
      productId: 0x2765,
      usbVersion: '2.0.0',
      deviceClass: 6,
      deviceSubclass: 1,
      deviceProtocol: 1,
      deviceVersion: '4.1.0',
      manufacturerName: 'Acme',
      productName: 'Phone',
    })
  })

  test('never carries a serial number, even when the browser object has one', () => {
    const fromBrowser = { ...phone, serialNumber: '24:0A:C4:00:00:01' }
    const described = describeUsbDevice(fromBrowser)
    expect(JSON.stringify(described)).not.toContain('24:0A')
    expect(Object.keys(described)).not.toContain('serialNumber')
  })

  test('passes the relay decoder, which rejects unknown fields', () => {
    const decode = Schema.decodeUnknownSync(UsbDescriptor, {
      onExcessProperty: 'error',
    })
    expect(() => decode(describeUsbDevice(phone))).not.toThrow()
  })

  test('leaves out names the device does not report, and clips long ones', () => {
    const described = describeUsbDevice({
      ...phone,
      manufacturerName: null,
      productName: 'x'.repeat(200),
    })
    expect(described.manufacturerName).toBeUndefined()
    expect(described.productName).toHaveLength(64)
    expect(Object.keys(described)).not.toContain('manufacturerName')
  })
})

describe('usbClassName', () => {
  test('names the class codes a visitor may see', () => {
    expect(usbClassName(6)).toBe('Still Image')
    expect(usbClassName(0xff)).toBe('Vendor Specific')
    expect(usbClassName(0x77)).toBe('Class 119')
  })

  test('the UI sentence format matches the lsusb style id', () => {
    expect(formatUsbId({ vendorId: 0x22d9, productId: 0x2765 })).toBe(
      '22d9:2765',
    )
  })
})
