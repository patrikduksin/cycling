import * as Array from 'effect/Array'

import type { Direction, Monitor, Row } from './model.ts'

export const MONITOR = { maxRows: 100, rowBytes: 192 } as const

export const emptyMonitor: Monitor = {
  rows: [],
  nextRowId: 0,
  bytesFromDevice: 0,
  bytesToDevice: 0,
  droppedFromDevice: 0,
  droppedToDevice: 0,
}

const hexByte = (byte: number): string => byte.toString(16).padStart(2, '0')

const isPrintable = (byte: number): boolean => byte >= 0x20 && byte <= 0x7e

/** One character per byte, so the text lines up with the hex column. */
const textByte = (byte: number): string =>
  isPrintable(byte) ? String.fromCharCode(byte) : '·'

export const makeRow = (
  id: number,
  direction: Direction,
  bytes: Uint8Array,
): Row => {
  const shown = bytes.subarray(0, MONITOR.rowBytes)
  return {
    id,
    direction,
    size: bytes.length,
    hex: Array.map(Array.fromIterable(shown), hexByte).join(' '),
    text: Array.map(Array.fromIterable(shown), textByte).join(''),
    hiddenBytes: bytes.length - shown.length,
  }
}

export interface Sample {
  readonly direction: Direction
  readonly bytes: Uint8Array
  readonly dropped: number
}

export const record = (monitor: Monitor, sample: Sample): Monitor => {
  const rows =
    sample.bytes.length === 0
      ? monitor.rows
      : Array.takeRight(
          [
            ...monitor.rows,
            makeRow(monitor.nextRowId, sample.direction, sample.bytes),
          ],
          MONITOR.maxRows,
        )
  const fromDevice = sample.direction === 'fromDevice'
  return {
    rows,
    nextRowId:
      sample.bytes.length === 0 ? monitor.nextRowId : monitor.nextRowId + 1,
    bytesFromDevice:
      monitor.bytesFromDevice + (fromDevice ? sample.bytes.length : 0),
    bytesToDevice:
      monitor.bytesToDevice + (fromDevice ? 0 : sample.bytes.length),
    droppedFromDevice:
      monitor.droppedFromDevice + (fromDevice ? sample.dropped : 0),
    droppedToDevice:
      monitor.droppedToDevice + (fromDevice ? 0 : sample.dropped),
  }
}
