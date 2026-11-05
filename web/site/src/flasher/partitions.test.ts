import { describe, expect, test } from 'bun:test'
import * as Option from 'effect/Option'

import {
  DEMO_PARTITIONS,
  encodeOtadata,
  encodePartitionTable,
} from '../dev/image.ts'
import type { Layout } from '../model.ts'
import {
  bootSlotOf,
  crc32,
  labelOf,
  otadataOf,
  parsePartitions,
  stakeOf,
} from './partitions.ts'

const layout = (bootSlot: string, bootloader = 0): Option.Option<Layout> =>
  Option.some({
    bootloader,
    partitions: DEMO_PARTITIONS,
    bootSlot: Option.some(bootSlot),
  })

describe('partition table', () => {
  test('reads every 0xAA50 entry and stops at erased flash', () => {
    expect(parsePartitions(encodePartitionTable(DEMO_PARTITIONS))).toEqual(
      DEMO_PARTITIONS,
    )
  })

  test('stops at the MD5 record gen_esp32part appends', () => {
    const table = encodePartitionTable(DEMO_PARTITIONS.slice(0, 2))
    table.set([0xeb, 0xeb], 64)
    expect(parsePartitions(table).map(entry => entry.label)).toEqual([
      'nvs',
      'otadata',
    ])
  })

  test('erased or foreign bytes have no partitions', () => {
    expect(parsePartitions(new Uint8Array(0xc00).fill(0xff))).toEqual([])
    expect(parsePartitions(new Uint8Array(10))).toEqual([])
  })

  test('finds otadata in the table', () => {
    expect(otadataOf(DEMO_PARTITIONS)).toEqual(
      Option.some({ address: 0xd000, size: 0x2000 }),
    )
    expect(otadataOf([])).toEqual(Option.none())
  })
})

describe('otadata', () => {
  test('crc matches python binascii.crc32(pack("<I", seq), 0xFFFFFFFF)', () => {
    expect(crc32(Uint8Array.of(1, 0, 0, 0), 0xffff_ffff)).toBe(0x4743_989a)
    expect(crc32(Uint8Array.of(2, 0, 0, 0), 0xffff_ffff)).toBe(0x55f6_3774)
  })

  test.each([
    [[1], 'ota_0'],
    [[2], 'ota_1'],
    [[3, 2], 'ota_0'],
    [[4, 5], 'ota_0'],
  ])('records %p boot %s', (seqs, slot) => {
    expect(bootSlotOf(DEMO_PARTITIONS, encodeOtadata(seqs))).toEqual(
      Option.some(slot),
    )
  })

  test('a record with a bad crc is ignored', () => {
    const otadata = encodeOtadata([1, 2])
    otadata.set([0x00], 0x1000 + 28)
    expect(bootSlotOf(DEMO_PARTITIONS, otadata)).toEqual(Option.some('ota_0'))
  })

  test('erased otadata boots factory, else the first slot', () => {
    const erased = encodeOtadata([])
    expect(bootSlotOf(DEMO_PARTITIONS, erased)).toEqual(Option.some('ota_0'))
    const factory = [
      ...DEMO_PARTITIONS,
      {
        label: 'factory',
        type: 0,
        subtype: 0,
        address: 0xf00000,
        size: 0x1000,
      },
    ]
    expect(bootSlotOf(factory, erased)).toEqual(Option.some('factory'))
  })
})

describe('region names', () => {
  test.each([
    [{ address: 0, size: 0x8000 }, 'the bootloader'],
    [{ address: 0x8000, size: 0xc00 }, 'the partition table'],
    [{ address: 0xd000, size: 0x2000 }, 'otadata'],
    [{ address: 0x760000, size: 0x1e0000 }, 'ota_1'],
    [{ address: 0, size: 16 * 1024 * 1024 }, 'the whole flash'],
  ])('%o is %s', (region, label) => {
    expect(labelOf(layout('ota_0'), region)).toEqual(Option.some(label))
  })

  test("the bootloader starts where the chip's family keeps it", () => {
    const esp32 = layout('ota_0', 0x1000)
    expect(labelOf(esp32, { address: 0x1000, size: 0x7000 })).toEqual(
      Option.some('the bootloader'),
    )
    expect(labelOf(esp32, { address: 0, size: 0x1000 })).toEqual(Option.none())
    expect(stakeOf(esp32, { address: 0, size: 0x1000 })._tag).toBe('Elsewhere')
    expect(stakeOf(esp32, { address: 0, size: 0x1001 })._tag).toBe('Bootloader')
    const p4 = layout('ota_0', 0x2000)
    expect(labelOf(p4, { address: 0x1000, size: 0x10 })).toEqual(Option.none())
    expect(labelOf(p4, { address: 0x2000, size: 0x10 })).toEqual(
      Option.some('the bootloader'),
    )
  })

  test('a region no partition holds has no name', () => {
    expect(
      labelOf(layout('ota_0'), { address: 0x750000, size: 0x20000 }),
    ).toEqual(Option.none())
    expect(labelOf(Option.none(), { address: 0x760000, size: 16 })).toEqual(
      Option.none(),
    )
  })
})

describe('what a write puts at stake', () => {
  test.each([
    [{ address: 0x1000, size: 0x10 }, 'Bootloader'],
    [{ address: 0x8000, size: 0xc00 }, 'PartitionTable'],
    [{ address: 0xd000, size: 0x1000 }, 'Otadata'],
    [{ address: 0x10000, size: 0x1000 }, 'BootSlot'],
    [{ address: 0x760000, size: 0x1e0000 }, 'Elsewhere'],
  ] as const)('%o: %s', (region, stake) => {
    expect(stakeOf(layout('ota_0'), region)._tag).toBe(stake)
  })

  test('with ota_1 selected, writing ota_1 is the boot slot', () => {
    expect(
      stakeOf(layout('ota_1'), { address: 0x760000, size: 0x1000 }),
    ).toEqual({ _tag: 'BootSlot', label: 'ota_1' })
  })

  test('without a layout only the fixed areas are known', () => {
    expect(stakeOf(Option.none(), { address: 0x760000, size: 16 })._tag).toBe(
      'Unknown',
    )
    expect(stakeOf(Option.none(), { address: 0, size: 16 })._tag).toBe(
      'Bootloader',
    )
  })
})
