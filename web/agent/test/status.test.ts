import { describe, expect, test } from 'bun:test'

import { statusLine } from '../src/Status.ts'
import { BAUD_115200, EPOCH_1, openStatus } from './support/fakePort.ts'

const status = {
  ...openStatus(EPOCH_1, BAUD_115200),
  traffic: { fromDevice: 5 * 1024 * 1024, toDevice: 912, dropped: 0 },
}

describe('status line', () => {
  test('counts traffic in B, KiB and MiB and says nothing of drops while there are none', () => {
    expect(statusLine({ status, ptyDropped: 0 })).toBe(
      '[ up  epoch 1  port open 115200  from device 5.0 MiB  to device 912 B ]',
    )
  })

  test('device bytes no pty reader took are counted as dropped', () => {
    expect(statusLine({ status, ptyDropped: 2048 })).toContain(
      'dropped 2.0 KiB',
    )
  })
})
