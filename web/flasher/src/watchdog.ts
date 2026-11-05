/**
 * The ESP32-S3 register writes esptool-js lacks, copied from python esptool
 * 5.4.0 `targets/esp32s3.py` (`disable_watchdogs`, `watchdog_reset`). A
 * devtools test parses this file and compares each table with that source,
 * so every number stays a plain literal.
 */
export type RegStep =
  | { readonly _tag: 'Write'; readonly address: number; readonly value: number }
  | {
      readonly _tag: 'SetBits'
      readonly address: number
      readonly bits: number
    }
  | { readonly _tag: 'Hold'; readonly ms: number }

export const DISABLE_WATCHDOGS: ReadonlyArray<RegStep> = [
  { _tag: 'Write', address: 0x600080b0, value: 0x50d83aa1 },
  { _tag: 'Write', address: 0x60008098, value: 0 },
  { _tag: 'Write', address: 0x600080b0, value: 0 },
  { _tag: 'Write', address: 0x600080b8, value: 0x8f1d312a },
  { _tag: 'SetBits', address: 0x600080b4, bits: 0x80000000 },
  { _tag: 'Write', address: 0x600080b8, value: 0 },
]

/** The chip resets about 15 ms after the third write, so the last write's reply may never come. */
export const WATCHDOG_RESET: ReadonlyArray<RegStep> = [
  { _tag: 'Write', address: 0x600080b0, value: 0x50d83aa1 },
  { _tag: 'Write', address: 0x6000809c, value: 2000 },
  { _tag: 'Write', address: 0x60008098, value: 0xd0000102 },
  { _tag: 'Write', address: 0x600080b0, value: 0 },
  { _tag: 'Hold', ms: 500 },
]
