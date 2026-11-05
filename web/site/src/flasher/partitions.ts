import * as Array from 'effect/Array'
import * as Option from 'effect/Option'

import { MAX_FLASH_BYTES, type Region } from '@cycling/protocol'

import type { Layout, Partition } from '../model.ts'

/** Where ESP-IDF puts the table, and the most of it there can be. */
export const PARTITION_TABLE: Region = { address: 0x8000, size: 0xc00 }

const TABLE_SECTOR = { start: 0x8000, end: 0x9000 }

/** Everything below the table boots the chip, whatever offset the family's bootloader starts at. */
const BOOT_SECTORS = { start: 0, end: TABLE_SECTOR.start }

/**
 * From where the chip's family keeps its bootloader (0 on the S3 and C3,
 * 0x1000 on the ESP32 and S2, 0x2000 on the C5 and P4) up to the table;
 * without a layout, from 0.
 */
const bootloaderOf = (layout: Option.Option<Layout>) => ({
  start: Option.match(layout, {
    onNone: () => 0,
    onSome: ({ bootloader }) => bootloader,
  }),
  end: TABLE_SECTOR.start,
})

const ENTRY_BYTES = 32
const LABEL_BYTES = 16
const TYPE_APP = 0x00
const TYPE_DATA = 0x01
const SUBTYPE_FACTORY = 0x00
const SUBTYPE_OTA_DATA = 0x00
const SUBTYPE_OTA_0 = 0x10
const SUBTYPE_OTA_LAST = 0x1f

const OTA_SECTOR_BYTES = 0x1000
const OTA_SEQ_UNSET = 0xffff_ffff
/** esp_ota_img_states_t: INVALID and ABORTED images are never booted. */
const OTA_STATE_INVALID = 0x3
const OTA_STATE_ABORTED = 0x4

const isEntry = (bytes: Uint8Array, at: number): boolean =>
  bytes[at] === 0xaa && bytes[at + 1] === 0x50

const decodeLabel = (bytes: Uint8Array): string => {
  const end = bytes.indexOf(0)
  return new TextDecoder().decode(end === -1 ? bytes : bytes.subarray(0, end))
}

/**
 * The entries of an ESP-IDF partition table: 32-byte records with magic
 * 0xAA50, read until the first record without it (the 0xEBEB MD5 record or
 * erased flash).
 */
export const parsePartitions = (
  bytes: Uint8Array,
): ReadonlyArray<Partition> => {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const entries: Array<Partition> = []
  for (
    let at = 0;
    at + ENTRY_BYTES <= bytes.byteLength && isEntry(bytes, at);
    at += ENTRY_BYTES
  ) {
    entries.push({
      type: view.getUint8(at + 2),
      subtype: view.getUint8(at + 3),
      address: view.getUint32(at + 4, true),
      size: view.getUint32(at + 8, true),
      label: decodeLabel(bytes.subarray(at + 12, at + 12 + LABEL_BYTES)),
    })
  }
  return entries
}

/** Where otadata is on this device, if it has one. */
export const otadataOf = (
  partitions: ReadonlyArray<Partition>,
): Option.Option<Region> =>
  Array.findFirst(
    partitions,
    entry => entry.type === TYPE_DATA && entry.subtype === SUBTYPE_OTA_DATA,
  ).pipe(
    Option.map(entry => ({
      address: entry.address,
      size: Math.min(entry.size, 2 * OTA_SECTOR_BYTES),
    })),
  )

const CRC_TABLE = Array.makeBy(256, n => {
  let c = n
  for (let bit = 0; bit < 8; bit++) {
    c = c & 1 ? 0xedb8_8320 ^ (c >>> 1) : c >>> 1
  }
  return c >>> 0
})

/** zlib's crc32 continuing from `seed`, as `binascii.crc32(bytes, seed)`. */
export const crc32 = (bytes: Uint8Array, seed: number): number => {
  let c = ~seed >>> 0
  for (const byte of bytes) {
    c = (CRC_TABLE[(c ^ byte) & 0xff] ?? 0) ^ (c >>> 8)
  }
  return ~c >>> 0
}

/**
 * One esp_ota_select_entry_t: seq, a 20-byte label, state, then the crc the
 * bootloader computes over seq alone, seeded with 0xFFFFFFFF.
 */
const validSeq = (record: Uint8Array): Option.Option<number> => {
  if (record.byteLength < 32) {
    return Option.none()
  }
  const view = new DataView(record.buffer, record.byteOffset, 32)
  const seq = view.getUint32(0, true)
  const state = view.getUint32(24, true)
  const crc = view.getUint32(28, true)
  return seq === OTA_SEQ_UNSET ||
    state === OTA_STATE_INVALID ||
    state === OTA_STATE_ABORTED ||
    crc !== crc32(record.subarray(0, 4), 0xffff_ffff)
    ? Option.none()
    : Option.some(seq)
}

const isOtaApp = (entry: Partition): boolean =>
  entry.type === TYPE_APP &&
  entry.subtype >= SUBTYPE_OTA_0 &&
  entry.subtype <= SUBTYPE_OTA_LAST

/**
 * The app the bootloader starts: the OTA slot the newest valid otadata
 * record selects, else factory, else the first OTA slot.
 */
export const bootSlotOf = (
  partitions: ReadonlyArray<Partition>,
  otadata: Uint8Array,
): Option.Option<string> => {
  const apps = partitions.filter(isOtaApp)
  const seqs = Array.getSomes([
    validSeq(otadata.subarray(0, 32)),
    validSeq(otadata.subarray(OTA_SECTOR_BYTES, OTA_SECTOR_BYTES + 32)),
  ])
  const bySlot = (slot: number) =>
    Array.findFirst(apps, entry => entry.subtype === SUBTYPE_OTA_0 + slot)
  const fallback = Array.findFirst(
    partitions,
    entry => entry.type === TYPE_APP && entry.subtype === SUBTYPE_FACTORY,
  ).pipe(Option.orElse(() => bySlot(0)))
  const chosen =
    seqs.length === 0 || apps.length === 0
      ? fallback
      : bySlot((Math.max(...seqs) - 1) % apps.length)
  return Option.map(chosen, entry => entry.label)
}

const contains = (
  span: { readonly start: number; readonly end: number },
  region: Region,
): boolean =>
  region.address >= span.start && region.address + region.size <= span.end

const overlaps = (
  span: { readonly start: number; readonly end: number },
  region: Region,
): boolean =>
  region.address < span.end && span.start < region.address + region.size

const spanOf = (entry: {
  readonly address: number
  readonly size: number
}) => ({
  start: entry.address,
  end: entry.address + entry.size,
})

/** The visitor's name for a region: a partition that holds it, or one of the fixed areas. */
export const labelOf = (
  layout: Option.Option<Layout>,
  region: Region,
): Option.Option<string> => {
  if (region.address === 0 && region.size === MAX_FLASH_BYTES) {
    return Option.some('the whole flash')
  }
  if (contains(bootloaderOf(layout), region)) {
    return Option.some('the bootloader')
  }
  if (contains(TABLE_SECTOR, region)) {
    return Option.some('the partition table')
  }
  return layout.pipe(
    Option.flatMap(({ partitions }) =>
      Array.findFirst(partitions, entry => contains(spanOf(entry), region)),
    ),
    Option.map(entry => entry.label),
  )
}

/**
 * What a write to `region` puts at stake, worst first. `unknown` when the
 * page has no layout to check a claim against.
 */
export type Stake =
  | { readonly _tag: 'Bootloader' }
  | { readonly _tag: 'PartitionTable' }
  | { readonly _tag: 'Otadata' }
  | { readonly _tag: 'BootSlot'; readonly label: string }
  | { readonly _tag: 'Elsewhere' }
  | { readonly _tag: 'Unknown' }

export const stakeOf = (
  layout: Option.Option<Layout>,
  region: Region,
): Stake => {
  if (overlaps(BOOT_SECTORS, region)) {
    return { _tag: 'Bootloader' }
  }
  if (overlaps(TABLE_SECTOR, region)) {
    return { _tag: 'PartitionTable' }
  }
  return Option.match(layout, {
    onNone: (): Stake => ({ _tag: 'Unknown' }),
    onSome: ({ partitions, bootSlot }): Stake => {
      const touched = partitions.filter(entry =>
        overlaps(spanOf(entry), region),
      )
      if (
        touched.some(
          entry =>
            entry.type === TYPE_DATA && entry.subtype === SUBTYPE_OTA_DATA,
        )
      ) {
        return { _tag: 'Otadata' }
      }
      return Option.match(bootSlot, {
        onNone: (): Stake => ({ _tag: 'Unknown' }),
        onSome: label =>
          touched.some(entry => entry.label === label)
            ? { _tag: 'BootSlot', label }
            : { _tag: 'Elsewhere' },
      })
    },
  })
}
