/**
 * The chips the simulator can be, each on the link a real board puts it
 * on. Register addresses come from the TRMs and python esptool's target
 * classes, not from the flasher, so a wrong table in the flasher fails here.
 */
export type SimChip = 'ESP32-S3' | 'ESP32-C3' | 'ESP32'

/** The RTC watchdog and super watchdog registers, and their write-protect keys. */
export interface Watchdogs {
  readonly config0: number
  readonly config1: number
  readonly wprotect: number
  readonly swdConf: number
  readonly swdWprotect: number
  readonly swdAutoFeed: number
}

export interface Profile {
  /** What Web Serial reports: USB-Serial-JTAG (303a:1001), or a CP2102N bridge. */
  readonly usb: SerialPortInfo
  /** The USB device leaves the bus on a reset; a UART bridge never does. */
  readonly reenumerates: boolean
  /** GET_SECURITY_INFO's chip id; undefined where the ROM has no such command. */
  readonly chipId: number | undefined
  /** What CHIP_DETECT_MAGIC_REG (0x40001000) reads. */
  readonly magic: number
  /** The eFuse words esptool-js reads the MAC from: low four bytes, then the high two. */
  readonly macWords: readonly [number, number]
  /** The reset cause the ROM prints after RTS released EN. */
  readonly pinReset: string
  /** Present where the USB reset into the ROM leaves the RTC WDT armed. */
  readonly watchdogs: Watchdogs | undefined
  readonly spi: {
    readonly cmd: number
    readonly user2: number
    readonly w0: number
  }
  /** Registers that read a fixed value, such as the UART divider the ESP32's crystal is measured by. */
  readonly fixed: ReadonlyArray<readonly [number, number]>
  readonly downloadBanner: (cause: string) => string
  readonly appBanner: (cause: string) => string
}

const S3_ROM = 'ESP-ROM:esp32s3-20210327\r\nBuild:Mar 27 2021\r\n'
const C3_ROM = 'ESP-ROM:esp32c3-api1-20210207\r\nBuild:Feb  7 2021\r\n'
const ESP32_ROM = 'ets Jul 29 2019 12:21:46\r\n\r\n'

const idfApp = (rom: string, cause: string, boot: string): string =>
  `${rom}rst:${cause},boot:${boot}\r\nSPIWP:0xee\r\nmode:DIO, clock div:1\r\nentry 0x403c8b28\r\nI (27) boot: ESP-IDF v5.1.2 2nd stage bootloader\r\nI (31) boot: Loaded app from partition at offset 0x20000\r\nI (240) app: sim up\r\n`

const ESPRESSIF_USB_JTAG = { usbVendorId: 0x303a, usbProductId: 0x1001 }
const CP2102N = { usbVendorId: 0x10c4, usbProductId: 0xea60 }
const MAGIC_ESP32 = 0x00f01d83

export const PROFILES: Readonly<Record<SimChip, Profile>> = {
  'ESP32-S3': {
    usb: ESPRESSIF_USB_JTAG,
    reenumerates: true,
    chipId: 9,
    magic: 0x9,
    macWords: [0x60007044, 0x60007048],
    pinReset: '0x15 (USB_UART_CHIP_RESET)',
    watchdogs: {
      config0: 0x60008098,
      config1: 0x6000809c,
      wprotect: 0x600080b0,
      swdConf: 0x600080b4,
      swdWprotect: 0x600080b8,
      swdAutoFeed: 0x80000000,
    },
    spi: { cmd: 0x60002000, user2: 0x60002020, w0: 0x60002058 },
    fixed: [],
    downloadBanner: cause =>
      `${S3_ROM}rst:${cause},boot:0x0 (DOWNLOAD(USB/UART0))\r\nSaved PC:0x40048036\r\nwaiting for download\r\n`,
    appBanner: cause => idfApp(S3_ROM, cause, '0x8 (SPI_FAST_FLASH_BOOT)'),
  },
  'ESP32-C3': {
    usb: ESPRESSIF_USB_JTAG,
    reenumerates: true,
    chipId: 5,
    magic: 0x1b31506f,
    macWords: [0x60008844, 0x60008848],
    pinReset: '0x15 (USB_UART_CHIP_RESET)',
    watchdogs: {
      config0: 0x60008090,
      config1: 0x60008094,
      wprotect: 0x600080a8,
      swdConf: 0x600080ac,
      swdWprotect: 0x600080b0,
      swdAutoFeed: 0x80000000,
    },
    spi: { cmd: 0x60002000, user2: 0x60002020, w0: 0x60002058 },
    fixed: [],
    downloadBanner: cause =>
      `${C3_ROM}rst:${cause},boot:0x0 (DOWNLOAD(USB/UART0/1))\r\nSaved PC:0x40048036\r\nwaiting for download\r\n`,
    appBanner: cause => idfApp(C3_ROM, cause, '0xc (SPI_FAST_FLASH_BOOT)'),
  },
  ESP32: {
    usb: CP2102N,
    reenumerates: false,
    chipId: undefined,
    magic: MAGIC_ESP32,
    macWords: [0x3ff5a004, 0x3ff5a008],
    pinReset: '0x1 (POWERON_RESET)',
    watchdogs: undefined,
    spi: { cmd: 0x3ff42000, user2: 0x3ff42024, w0: 0x3ff42080 },
    /** UART0_CLKDIV at 115200 baud from a 40 MHz crystal. */
    fixed: [[0x3ff40014, 347]],
    downloadBanner: cause =>
      `${ESP32_ROM}rst:${cause},boot:0x3 (DOWNLOAD_BOOT(UART0/UART1/SDIO_REI_REO_V2))\r\nwaiting for download\r\n`,
    appBanner: cause => idfApp(ESP32_ROM, cause, '0x13 (SPI_FAST_FLASH_BOOT)'),
  },
}
