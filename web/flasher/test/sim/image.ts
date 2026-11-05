import SparkMD5 from 'spark-md5'

import { FLASH_BYTES } from './device.ts'

/**
 * A 16 MiB C606 stock layout that devtools' `Device.connect(strict=True)`
 * accepts (`firmware.partitions` and `records`): the same shape as
 * `stock_flash()` in tools/devtools/tests/test_device.py.
 */
export const SLOT_A = 0x20000
export const SLOT_B = 0x760000
const SLOT_SIZE = 0x73a000
const TABLE = 0x8000
const OTADATA = 0xd000
const APP_DESC_MAGIC = 0xabcd5432

const PARTITIONS: ReadonlyArray<
  readonly [
    label: string,
    type: number,
    subtype: number,
    offset: number,
    size: number,
  ]
> = [
  ['nvs', 1, 2, 0x9000, 0x4000],
  ['otadata', 1, 0, 0xd000, 0x2000],
  ['phy_init', 1, 1, 0xf000, 0x1000],
  ['coredump', 1, 3, 0x10000, 0x10000],
  ['ota_0', 0, 0x10, SLOT_A, SLOT_SIZE],
  ['ota_1', 0, 0x11, SLOT_B, SLOT_SIZE],
]

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) {
    c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  }
  return c >>> 0
})

/** python's `zlib.crc32(data, value)`. */
const crc32 = (data: Uint8Array, value: number): number => {
  let c = (value ^ 0xffffffff) >>> 0
  for (const byte of data) {
    c = ((CRC_TABLE[(c ^ byte) & 0xff] ?? 0) ^ (c >>> 8)) >>> 0
  }
  return (c ^ 0xffffffff) >>> 0
}

const md5Bytes = (bytes: Uint8Array): Uint8Array =>
  Uint8Array.from(
    SparkMD5.ArrayBuffer.hash(bytes.slice().buffer).match(/../g) ?? [],
    pair => parseInt(pair, 16),
  )

/** Everything but slot A's image: bootloader stand-in, partition table, otadata selecting ota_0, slot B leftovers. */
export const stockLayout = (): Uint8Array => {
  const flash = new Uint8Array(FLASH_BYTES).fill(0xff)
  for (let at = 0; at < TABLE; at++) {
    flash[at] = at & 0xff
  }
  const table = new DataView(flash.buffer, TABLE, 0x1000)
  PARTITIONS.forEach(([label, type, subtype, offset, size], index) => {
    const at = index * 32
    table.setUint16(at, 0x50aa, true)
    table.setUint8(at + 2, type)
    table.setUint8(at + 3, subtype)
    table.setUint32(at + 4, offset, true)
    table.setUint32(at + 8, size, true)
    flash.fill(0, TABLE + at + 12, TABLE + at + 28)
    flash.set(new TextEncoder().encode(label), TABLE + at + 12)
    table.setUint32(at + 28, 0, true)
  })
  const end = PARTITIONS.length * 32
  flash.set([0xeb, 0xeb], TABLE + end)
  flash.set(md5Bytes(flash.subarray(TABLE, TABLE + end)), TABLE + end + 16)

  const record = new DataView(flash.buffer, OTADATA, 32)
  record.setUint32(0, 1, true)
  record.setUint32(24, 0xffffffff, true)
  record.setUint32(
    28,
    crc32(flash.subarray(OTADATA, OTADATA + 4), 0xffffffff),
    true,
  )

  flash.fill(0x5a, SLOT_B, SLOT_B + 0x3000)
  return flash
}

/** A valid stock ESP32-S3 application (not cycling): one segment, checksum, appended SHA-256. */
export const stockImage = async (): Promise<Uint8Array> => {
  const segment = new Uint8Array(0x60 + 0x1800)
  const descriptor = new DataView(segment.buffer)
  descriptor.setUint32(0, APP_DESC_MAGIC, true)
  segment.set(new TextEncoder().encode('0.1.0'), 0x10)
  segment.set(new TextEncoder().encode('Project'), 0x30)
  const header = new Uint8Array(24 + 8)
  const view = new DataView(header.buffer)
  view.setUint8(0, 0xe9)
  view.setUint8(1, 1)
  view.setUint16(12, 9, true)
  view.setUint8(23, 1)
  view.setUint32(24, 0x3c000020, true)
  view.setUint32(28, segment.length, true)
  const unpadded = header.length + segment.length
  const end = (unpadded + 16) & ~15
  const body = new Uint8Array(end)
  body.set(header)
  body.set(segment, header.length)
  body[end - 1] = segment.reduce((state, byte) => state ^ byte, 0xef)
  const sha = new Uint8Array(await crypto.subtle.digest('SHA-256', body))
  const image = new Uint8Array(end + sha.length)
  image.set(body)
  image.set(sha, end)
  return image
}

export const stockFlash = async (): Promise<Uint8Array> => {
  const flash = stockLayout()
  flash.set(await stockImage(), SLOT_A)
  return flash
}
