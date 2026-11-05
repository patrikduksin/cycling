/**
 * What python esptool 5.4.0 does per chip family and esptool-js 0.7.1 does
 * not. A devtools test (`tests/test_flasher_watchdog_tables.py`) runs
 * python's own `_post_connect`, `watchdog_reset` and `hard_reset` for every
 * family against a recording chip and port and compares them with these
 * tables, so every number stays a plain literal.
 */
export type Step =
  /** WRITE_REG; `mask` defaults to every bit. */
  | {
      readonly _tag: 'Write'
      readonly address: number
      readonly value: number
      readonly mask?: number
    }
  /** READ_REG, then WRITE_REG of the value with `bits` set. */
  | {
      readonly _tag: 'SetBits'
      readonly address: number
      readonly bits: number
    }
  | { readonly _tag: 'Hold'; readonly ms: number }
  /** A modem line, as pyserial's `setRTS(asserted)` (EN low on the usual auto-reset circuit). */
  | { readonly _tag: 'Rts'; readonly asserted: boolean }
  /** pyserial's `setDTR(asserted)` (GPIO0 low on the usual auto-reset circuit). */
  | { readonly _tag: 'Dtr'; readonly asserted: boolean }

export interface Family {
  /** The ROM answers GET_SECURITY_INFO; ESP8266 and ESP32 have no such command. */
  readonly securityInfo: boolean
  /** `_post_connect` on USB-Serial-JTAG: the RTC and super watchdogs the USB reset into the ROM leaves running. */
  readonly disableWatchdogs: ReadonlyArray<Step>
  /** `watchdog_reset` (`--after watchdog-reset`) on the chip's own USB, Serial-JTAG or OTG; where python has none it falls back to `hard_reset`. */
  readonly usbReboot: ReadonlyArray<Step>
  /** What `hard_reset` does before its RTS pulse on a USB-UART bridge. */
  readonly hardReset: ReadonlyArray<Step>
}

/** esp_pylib's `hard_reset` on a UART bridge: EN held low 100 ms. Setting RTS re-sends DTR, as python does for Windows' usbser.sys. */
export const RTS_PULSE: ReadonlyArray<Step> = [
  { _tag: 'Rts', asserted: true },
  { _tag: 'Dtr', asserted: false },
  { _tag: 'Hold', ms: 100 },
  { _tag: 'Rts', asserted: false },
  { _tag: 'Dtr', asserted: false },
]

/**
 * The same on a bridge whose CTS follows the chip's RTS (the CP2102C,
 * 10c4:ea64), as python runs it where it cannot clear HUPCL: DTR keeps EN's
 * edge clean, then goes high again before the port closes.
 */
export const RTS_PULSE_FLOW_CONTROL: ReadonlyArray<Step> = [
  { _tag: 'Dtr', asserted: false },
  { _tag: 'Rts', asserted: true },
  { _tag: 'Dtr', asserted: false },
  { _tag: 'Hold', ms: 100 },
  { _tag: 'Dtr', asserted: true },
  { _tag: 'Hold', ms: 100 },
  { _tag: 'Rts', asserted: false },
  { _tag: 'Dtr', asserted: true },
  { _tag: 'Dtr', asserted: false },
]

/** esp_pylib's `HARDWARE_FLOW_CONTROL_VID_PIDS`: the bridges that take `RTS_PULSE_FLOW_CONTROL`. */
export const FLOW_CONTROL_BRIDGES: ReadonlyArray<{
  readonly vendorId: number
  readonly productId: number
}> = [{ vendorId: 0x10c4, productId: 0xea64 }]

/** Keyed by esptool-js's `CHIP_NAME`, which is python's too. */
export const FAMILIES = {
  ESP8266: {
    securityInfo: false,
    disableWatchdogs: [],
    usbReboot: [...RTS_PULSE],
    hardReset: [],
  },
  ESP32: {
    securityInfo: false,
    disableWatchdogs: [],
    usbReboot: [...RTS_PULSE],
    hardReset: [],
  },
  'ESP32-S2': {
    securityInfo: true,
    disableWatchdogs: [],
    usbReboot: [
      { _tag: 'Write', address: 0x3f4080ac, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x3f408098, value: 2000 },
      { _tag: 'Write', address: 0x3f408094, value: 0xd0000102 },
      { _tag: 'Write', address: 0x3f4080ac, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [],
  },
  'ESP32-S3': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x600080b0, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x60008098, value: 0 },
      { _tag: 'Write', address: 0x600080b0, value: 0 },
      { _tag: 'Write', address: 0x600080b8, value: 0x8f1d312a },
      { _tag: 'SetBits', address: 0x600080b4, bits: 0x80000000 },
      { _tag: 'Write', address: 0x600080b8, value: 0 },
    ],
    usbReboot: [
      { _tag: 'Write', address: 0x600080b0, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x6000809c, value: 2000 },
      { _tag: 'Write', address: 0x60008098, value: 0xd0000102 },
      { _tag: 'Write', address: 0x600080b0, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [{ _tag: 'Write', address: 0x6000812c, value: 0, mask: 0x1 }],
  },
  'ESP32-C3': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x600080a8, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x60008090, value: 0 },
      { _tag: 'Write', address: 0x600080a8, value: 0 },
      { _tag: 'Write', address: 0x600080b0, value: 0x8f1d312a },
      { _tag: 'SetBits', address: 0x600080ac, bits: 0x80000000 },
      { _tag: 'Write', address: 0x600080b0, value: 0 },
    ],
    usbReboot: [
      { _tag: 'Write', address: 0x600080a8, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x60008094, value: 2000 },
      { _tag: 'Write', address: 0x60008090, value: 0xd0000102 },
      { _tag: 'Write', address: 0x600080a8, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [],
  },
  'ESP32-C2': {
    securityInfo: true,
    disableWatchdogs: [],
    usbReboot: [
      { _tag: 'Write', address: 0x6000809c, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x60008088, value: 2000 },
      { _tag: 'Write', address: 0x60008084, value: 0xd0000102 },
      { _tag: 'Write', address: 0x6000809c, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [],
  },
  'ESP32-C6': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x600b1c18, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x600b1c00, value: 0 },
      { _tag: 'Write', address: 0x600b1c18, value: 0 },
      { _tag: 'Write', address: 0x600b1c20, value: 0x50d83aa1 },
      { _tag: 'SetBits', address: 0x600b1c1c, bits: 0x40000 },
      { _tag: 'Write', address: 0x600b1c20, value: 0 },
    ],
    usbReboot: [...RTS_PULSE],
    hardReset: [],
  },
  'ESP32-C61': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x600b1c18, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x600b1c00, value: 0 },
      { _tag: 'Write', address: 0x600b1c18, value: 0 },
      { _tag: 'Write', address: 0x600b1c20, value: 0x50d83aa1 },
      { _tag: 'SetBits', address: 0x600b1c1c, bits: 0x40000 },
      { _tag: 'Write', address: 0x600b1c20, value: 0 },
    ],
    usbReboot: [
      { _tag: 'Write', address: 0x600b1c18, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x600b1c04, value: 2000 },
      { _tag: 'Write', address: 0x600b1c00, value: 0xd0000102 },
      { _tag: 'Write', address: 0x600b1c18, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [],
  },
  'ESP32-C5': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x600b1c18, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x600b1c00, value: 0 },
      { _tag: 'Write', address: 0x600b1c18, value: 0 },
      { _tag: 'Write', address: 0x600b1c20, value: 0x50d83aa1 },
      { _tag: 'SetBits', address: 0x600b1c1c, bits: 0x40000 },
      { _tag: 'Write', address: 0x600b1c20, value: 0 },
    ],
    usbReboot: [
      { _tag: 'Write', address: 0x600b1c18, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x600b1c04, value: 2000 },
      { _tag: 'Write', address: 0x600b1c00, value: 0xd0000102 },
      { _tag: 'Write', address: 0x600b1c18, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [],
  },
  'ESP32-E22': {
    securityInfo: true,
    disableWatchdogs: [],
    usbReboot: [...RTS_PULSE],
    hardReset: [],
  },
  'ESP32-H2': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x600b1c1c, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x600b1c00, value: 0 },
      { _tag: 'Write', address: 0x600b1c1c, value: 0 },
      { _tag: 'Write', address: 0x600b1c24, value: 0x50d83aa1 },
      { _tag: 'SetBits', address: 0x600b1c20, bits: 0x40000 },
      { _tag: 'Write', address: 0x600b1c24, value: 0 },
    ],
    usbReboot: [...RTS_PULSE],
    hardReset: [],
  },
  'ESP32-H21': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x600b1c1c, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x600b1c00, value: 0 },
      { _tag: 'Write', address: 0x600b1c1c, value: 0 },
      { _tag: 'Write', address: 0x600b1c24, value: 0x50d83aa1 },
      { _tag: 'SetBits', address: 0x600b1c20, bits: 0x40000 },
      { _tag: 'Write', address: 0x600b1c24, value: 0 },
    ],
    usbReboot: [...RTS_PULSE],
    hardReset: [],
  },
  'ESP32-P4': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x50116018, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x50116000, value: 0 },
      { _tag: 'Write', address: 0x50116018, value: 0 },
      { _tag: 'Write', address: 0x50116020, value: 0x50d83aa1 },
      { _tag: 'SetBits', address: 0x5011601c, bits: 0x40000 },
      { _tag: 'Write', address: 0x50116020, value: 0 },
    ],
    usbReboot: [
      { _tag: 'Write', address: 0x50116018, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x50116004, value: 2000 },
      { _tag: 'Write', address: 0x50116000, value: 0xd0000102 },
      { _tag: 'Write', address: 0x50116018, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [],
  },
  'ESP32-H4': {
    securityInfo: true,
    disableWatchdogs: [
      { _tag: 'Write', address: 0x60090064, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x60090048, value: 0 },
      { _tag: 'Write', address: 0x60090064, value: 0 },
      { _tag: 'Write', address: 0x600b5420, value: 0x50d83aa1 },
      { _tag: 'SetBits', address: 0x600b541c, bits: 0x40000 },
      { _tag: 'Write', address: 0x600b5420, value: 0 },
    ],
    usbReboot: [...RTS_PULSE],
    hardReset: [],
  },
  'ESP32-S31': {
    securityInfo: true,
    disableWatchdogs: [],
    usbReboot: [
      { _tag: 'Write', address: 0x20801018, value: 0x50d83aa1 },
      { _tag: 'Write', address: 0x20801004, value: 2000 },
      { _tag: 'Write', address: 0x20801000, value: 0xd0000102 },
      { _tag: 'Write', address: 0x20801018, value: 0 },
      { _tag: 'Hold', ms: 500 },
    ],
    hardReset: [],
  },
} as const satisfies Record<string, Family>

export type ChipFamily = keyof typeof FAMILIES

export const isChipFamily = (name: string): name is ChipFamily =>
  Object.hasOwn(FAMILIES, name)
