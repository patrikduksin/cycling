import { describe, expect, test } from 'bun:test'

import { MONITOR, emptyMonitor, makeRow, record } from './monitor.ts'

describe('makeRow', () => {
  test('shows hex and printable text, dotting what is not printable', () => {
    const row = makeRow(
      4,
      'fromDevice',
      Uint8Array.of(0x48, 0x69, 0x0a, 0x00, 0xff, 0x0d),
    )
    expect(row).toEqual({
      id: 4,
      direction: 'fromDevice',
      size: 6,
      hex: '48 69 0a 00 ff 0d',
      text: 'Hi····',
      hiddenBytes: 0,
    })
  })

  test('cuts a long batch at the row limit and says how much is hidden', () => {
    const row = makeRow(
      0,
      'toDevice',
      new Uint8Array(MONITOR.rowBytes + 10).fill(0x41),
    )
    expect(row.size).toBe(MONITOR.rowBytes + 10)
    expect(row.hiddenBytes).toBe(10)
    expect(row.text).toHaveLength(MONITOR.rowBytes)
  })
})

describe('record', () => {
  test('counts bytes and drops per direction and numbers the rows', () => {
    const first = record(emptyMonitor, {
      direction: 'fromDevice',
      bytes: Uint8Array.of(1, 2, 3),
      dropped: 0,
    })
    const second = record(first, {
      direction: 'toDevice',
      bytes: Uint8Array.of(4),
      dropped: 1,
    })
    expect(second).toMatchObject({
      bytesFromDevice: 3,
      bytesToDevice: 1,
      droppedFromDevice: 0,
      droppedToDevice: 1,
      nextRowId: 2,
    })
    expect(second.rows.map(row => row.id)).toEqual([0, 1])
  })

  test('keeps only the newest rows, but the counts never wrap', () => {
    const filled = Array.from(
      { length: MONITOR.maxRows + 25 },
      (_, index) => index,
    ).reduce(
      (monitor, index) =>
        record(monitor, {
          direction: 'fromDevice',
          bytes: Uint8Array.of(index % 256),
          dropped: 0,
        }),
      emptyMonitor,
    )
    expect(filled.rows).toHaveLength(MONITOR.maxRows)
    expect(filled.rows[0]?.id).toBe(25)
    expect(filled.rows.at(-1)?.id).toBe(MONITOR.maxRows + 24)
    expect(filled.bytesFromDevice).toBe(MONITOR.maxRows + 25)
  })

  test('a batch of only drops counts but adds no row', () => {
    const next = record(emptyMonitor, {
      direction: 'toDevice',
      bytes: new Uint8Array(0),
      dropped: 7,
    })
    expect(next.rows).toEqual([])
    expect(next.droppedToDevice).toBe(7)
  })
})
