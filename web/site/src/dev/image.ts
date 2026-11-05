import { MAX_FLASH_BYTES } from '@cycling/protocol'

import { crc32 } from '../flasher/partitions.ts'
import type { Partition } from '../model.ts'

/**
 * A made-up ESP-IDF layout with two app slots, for tests and `?fake=flash`.
 * It is not any real device's table.
 */
export const DEMO_PARTITIONS: ReadonlyArray<Partition> = [
  { label: 'nvs', type: 0x01, subtype: 0x02, address: 0x9000, size: 0x4000 },
  {
    label: 'otadata',
    type: 0x01,
    subtype: 0x00,
    address: 0xd000,
    size: 0x2000,
  },
  {
    label: 'phy_init',
    type: 0x01,
    subtype: 0x01,
    address: 0xf000,
    size: 0x1000,
  },
  {
    label: 'ota_0',
    type: 0x00,
    subtype: 0x10,
    address: 0x10000,
    size: 0x750000,
  },
  {
    label: 'ota_1',
    type: 0x00,
    subtype: 0x11,
    address: 0x760000,
    size: 0x750000,
  },
  {
    label: 'storage',
    type: 0x01,
    subtype: 0x82,
    address: 0xeb0000,
    size: 0x150000,
  },
]

/** The table as gen_esp32part.py writes it, erased flash after the last entry. */
export const encodePartitionTable = (
  partitions: ReadonlyArray<Partition>,
): Uint8Array => {
  const out = new Uint8Array(0xc00).fill(0xff)
  const view = new DataView(out.buffer)
  partitions.forEach((entry, index) => {
    const at = index * 32
    out.set([0xaa, 0x50, entry.type, entry.subtype], at)
    view.setUint32(at + 4, entry.address, true)
    view.setUint32(at + 8, entry.size, true)
    out.fill(0, at + 12, at + 28)
    out.set(new TextEncoder().encode(entry.label).subarray(0, 16), at + 12)
    view.setUint32(at + 28, 0, true)
  })
  return out
}

/** Two otadata sectors; `seqs` fills the first records, erased otherwise. */
export const encodeOtadata = (seqs: ReadonlyArray<number>): Uint8Array => {
  const out = new Uint8Array(0x2000).fill(0xff)
  const view = new DataView(out.buffer)
  seqs.forEach((seq, sector) => {
    const at = sector * 0x1000
    view.setUint32(at, seq, true)
    out.fill(0, at + 4, at + 24)
    view.setUint32(at + 24, 0x2, true)
    view.setUint32(at + 28, crc32(out.subarray(at, at + 4), 0xffff_ffff), true)
  })
  return out
}

/** 16 MiB of erased flash holding the demo table, with otadata selecting ota_0. */
export const demoFlash = (): Uint8Array => {
  const flash = new Uint8Array(MAX_FLASH_BYTES).fill(0xff)
  flash.set(encodePartitionTable(DEMO_PARTITIONS), 0x8000)
  flash.set(encodeOtadata([1]), 0xd000)
  return flash
}
