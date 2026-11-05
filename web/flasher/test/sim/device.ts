import { Inflate } from 'pako'
import SparkMD5 from 'spark-md5'

import { FakeSerialPort, type Enumeration, type Wire } from './port.ts'
import { SlipDecoder, slipEncode } from './slip.ts'

export const FLASH_BYTES = 16 * 1024 * 1024

export interface SimOptions {
  /** Defaults to 0xFF everywhere (erased). */
  readonly flash?: Uint8Array
  /** The eFuse MAC; use 02:00:00:xx:xx:xx. */
  readonly mac?: string
  /** After a USB reset from the app, the RTC WDT resets the chip this long after entry unless disabled. */
  readonly watchdogMs?: number
  /** How long the chip is off the bus after a reset that drops it. */
  readonly reenumerateMs?: number
  /** The USB reset into download mode also drops the port and re-enumerates. */
  readonly reenumerateOnEntry?: boolean
  readonly secureBoot?: boolean
  readonly flashCryptCnt?: number
  /** Every reply reaches the host this much later. */
  readonly replyDelayMs?: number
}

export type Mode = 'app' | 'held' | 'rom' | 'stub' | 'unplugged'

export interface SimState {
  readonly mode: Mode
  /** Resets that took the chip off the bus, watchdog or USB. */
  readonly resets: number
  readonly lastReset: string
  /** The RTC WDT armed by entry from the app is still due to fire. */
  readonly watchdogArmed: boolean
  readonly portOpen: boolean
  readonly enumerations: number
}

const OP = {
  SYNC: 0x08,
  WRITE_REG: 0x09,
  READ_REG: 0x0a,
  SPI_SET_PARAMS: 0x0b,
  SPI_ATTACH: 0x0d,
  CHANGE_BAUDRATE: 0x0f,
  MEM_BEGIN: 0x05,
  MEM_END: 0x06,
  MEM_DATA: 0x07,
  FLASH_DEFL_BEGIN: 0x10,
  FLASH_DEFL_DATA: 0x11,
  FLASH_DEFL_END: 0x12,
  SPI_FLASH_MD5: 0x13,
  GET_SECURITY_INFO: 0x14,
  ERASE_FLASH: 0xd0,
  ERASE_REGION: 0xd1,
  READ_FLASH: 0xd2,
} as const

/** Error codes in the status bytes: the ROM's, then the stub's. */
const ROM_INVALID_MESSAGE = 0x05
const ROM_FAILED = 0x06
const ROM_BAD_CHECKSUM = 0x07
const STUB_BAD_DATA_LEN = 0xc0
const STUB_BAD_CHECKSUM = 0xc1
const STUB_INVALID_COMMAND = 0xc5
const STUB_INFLATE_ERROR = 0xc6

/**
 * The sim's own register map, written from the ESP32-S3 TRM names rather than
 * imported from the flasher, so a wrong table in the flasher fails here.
 */
const REG = {
  MAC0: 0x60007044,
  MAC1: 0x60007048,
  RTC_WDT_CONFIG0: 0x60008098,
  RTC_WDT_CONFIG1: 0x6000809c,
  RTC_WDT_WPROTECT: 0x600080b0,
  SWD_CONF: 0x600080b4,
  SWD_WPROTECT: 0x600080b8,
} as const
const RTC_WDT_KEY = 0x50d83aa1
const SWD_KEY = 0x8f1d312a
const ENABLE = 0x80000000
const SWD_AUTO_FEED = 0x80000000
/** Stage 0 of the armed RTC WDT at 2000 slow-clock cycles. */
const WATCHDOG_ARM_MS = 15
const CHECKSUM_SEED = 0xef
const PENDING_LIMIT = 4096

const ROM_BANNER = 'ESP-ROM:esp32s3-20210327\r\nBuild:Mar 27 2021\r\n'

const downloadBanner = (cause: string): string =>
  `${ROM_BANNER}rst:${cause},boot:0x0 (DOWNLOAD(USB/UART0))\r\nSaved PC:0x40048036\r\nwaiting for download\r\n`

const appBanner = (cause: string): string =>
  `${ROM_BANNER}rst:${cause},boot:0x8 (SPI_FAST_FLASH_BOOT)\r\nSaved PC:0x403cd8c2\r\nSPIWP:0xee\r\nmode:DIO, clock div:1\r\nload:0x3fce2810,len:0x178c\r\nentry 0x403c8b28\r\nI (27) boot: ESP-IDF v5.1.2 2nd stage bootloader\r\nI (31) boot: Loaded app from partition at offset 0x20000\r\nI (240) app: sim-c606 up\r\n`

const text = new TextEncoder()

const u32 = (bytes: Uint8Array, at: number): number =>
  bytes.length < at + 4
    ? 0
    : new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(
        at,
        true,
      )

const le32 = (value: number): Array<number> => [
  value & 0xff,
  (value >>> 8) & 0xff,
  (value >>> 16) & 0xff,
  (value >>> 24) & 0xff,
]

const checksum = (data: Uint8Array): number =>
  data.reduce((state, byte) => state ^ byte, CHECKSUM_SEED)

const md5 = (bytes: Uint8Array): string =>
  SparkMD5.ArrayBuffer.hash(bytes.slice().buffer)

const hexBytes = (hex: string): Uint8Array =>
  Uint8Array.from(hex.match(/../g) ?? [], pair => parseInt(pair, 16))

const macWords = (mac: string): { mac0: number; mac1: number } => {
  const bytes = mac.split(':').map(part => parseInt(part, 16))
  const [b0 = 0, b1 = 0, b2 = 0, b3 = 0, b4 = 0, b5 = 0] = bytes
  return {
    mac0: ((b2 << 24) | (b3 << 16) | (b4 << 8) | b5) >>> 0,
    mac1: (b0 << 8) | b1,
  }
}

interface Reading {
  readonly address: number
  readonly size: number
  readonly packet: number
  readonly window: number
  /** Absolute flash address whose byte is flipped in transit, if any. */
  readonly corruptAt: number | undefined
  sent: number
  acked: number
}

interface Writing {
  readonly inflate: Inflate
  readonly blocks: number
  seq: number
}

interface Upload {
  readonly size: number
  readonly blocks: number
  received: number
  seq: number
}

interface ReadFault {
  readonly address: number
  readonly size: number
  times: number
}

type Listener = (port: FakeSerialPort) => void

/**
 * A fake ESP32-S3 on USB-Serial-JTAG: the app console, the ROM loader, the
 * flasher stub and the RTC watchdog, speaking exactly what esptool-js 0.7.1
 * sends. Everything runs on real timers, so dispose() it after a test.
 */
export class SimDevice implements Wire, Enumeration {
  readonly flash: Uint8Array
  private readonly options: Required<Omit<SimOptions, 'flash'>>
  private mode: Mode = 'app'
  /** What the chip ran before RTS held it in reset. */
  private heldFrom: Mode = 'app'
  private port: FakeSerialPort | undefined
  private signalState = { dtr: false, rts: false }
  private downloadStrap = false
  private registers = new Map<number, number>()
  private readonly decoder = new SlipDecoder()
  private consoleLine = ''
  private uptimeMs = 0
  private resets = 0
  private lastReset = 'POWERON'
  private enumerations = 0
  private silent = false
  private rtcWdtOff = false
  private swdFed = false
  private watchdog: ReturnType<typeof setTimeout> | undefined
  private reading: Reading | undefined
  private writing: Writing | undefined
  private upload: Upload | undefined
  private outbox: Array<Uint8Array> = []
  private pending: Array<Uint8Array> = []
  private pendingBytes = 0
  private readonly faults: Array<ReadFault> = []
  private readonly timers = new Set<ReturnType<typeof setTimeout>>()
  private readonly attachListeners: Array<Listener> = []
  private readonly detachListeners: Array<Listener> = []
  private uptime: ReturnType<typeof setInterval> | undefined

  constructor(options: SimOptions = {}) {
    this.flash = options.flash ?? new Uint8Array(FLASH_BYTES).fill(0xff)
    this.options = {
      mac: options.mac ?? '02:00:00:12:34:56',
      watchdogMs: options.watchdogMs ?? 8400,
      reenumerateMs: options.reenumerateMs ?? 300,
      reenumerateOnEntry: options.reenumerateOnEntry ?? false,
      secureBoot: options.secureBoot ?? false,
      flashCryptCnt: options.flashCryptCnt ?? 0,
      replyDelayMs: options.replyDelayMs ?? 0,
    }
    this.attach()
    this.bootApp('0x1 (POWERON)')
  }

  // ------------------------------------------------------------ test surface

  current(): FakeSerialPort | undefined {
    return this.port
  }

  onAttach(listener: Listener): void {
    this.attachListeners.push(listener)
  }

  onDetach(listener: Listener): void {
    this.detachListeners.push(listener)
  }

  /** Resolves with the next port to enumerate. */
  nextPort(): Promise<FakeSerialPort> {
    return new Promise(resolve => {
      const once: Listener = port => {
        this.attachListeners.splice(this.attachListeners.indexOf(once), 1)
        resolve(port)
      }
      this.attachListeners.push(once)
    })
  }

  state(): SimState {
    return {
      mode: this.mode,
      resets: this.resets,
      lastReset: this.lastReset,
      watchdogArmed: this.watchdog !== undefined,
      portOpen: this.port?.isOpen ?? false,
      enumerations: this.enumerations,
    }
  }

  flashMd5(address: number, size: number): string {
    return md5(this.flash.subarray(address, address + size))
  }

  /** The next `times` READ_FLASH commands overlapping the range carry one flipped byte. */
  corruptReads(address: number, size: number, times: number): void {
    this.faults.push({ address, size, times })
  }

  /** The cable is pulled: the port is lost and the chip stays off. */
  unplug(): void {
    this.stopChip()
    this.mode = 'unplugged'
    this.detach()
  }

  dispose(): void {
    this.stopChip()
    for (const timer of this.timers) {
      clearTimeout(timer)
    }
    this.timers.clear()
  }

  // -------------------------------------------------------------------- Wire

  opened(port: FakeSerialPort): void {
    if (port !== this.port) {
      return
    }
    for (const bytes of this.pending) {
      port.deliver(bytes)
    }
    this.pending = []
    this.pendingBytes = 0
  }

  closed(_port: FakeSerialPort): void {}

  receive(port: FakeSerialPort, bytes: Uint8Array): void {
    if (port !== this.port) {
      return
    }
    switch (this.mode) {
      case 'app':
        return this.consoleInput(bytes)
      case 'rom':
      case 'stub':
        for (const frame of this.decoder.push(bytes)) {
          this.frame(frame)
        }
        return
      case 'held':
      case 'unplugged':
        return
    }
  }

  /**
   * USB-Serial-JTAG maps RTS to chip reset and DTR to the boot strap: the
   * chip is held in reset while RTS is set and DTR clear, and leaves reset
   * into download mode when DTR was set (with RTS clear) since the last boot.
   */
  signals(signals: SerialOutputSignals): void {
    if (this.mode === 'unplugged') {
      return
    }
    this.signalState = {
      dtr: signals.dataTerminalReady ?? this.signalState.dtr,
      rts: signals.requestToSend ?? this.signalState.rts,
    }
    const { dtr, rts } = this.signalState
    if (dtr && !rts) {
      this.downloadStrap = true
    }
    if (rts && !dtr && this.mode !== 'held') {
      const from = this.mode
      this.stopChip({ keepWatchdog: true })
      this.mode = 'held'
      this.heldFrom = from
    } else if (!rts && this.mode === 'held') {
      this.releaseReset()
    }
  }

  // ------------------------------------------------------- resets and boots

  private releaseReset(): void {
    const download = this.downloadStrap
    const fromApp = this.heldFrom === 'app'
    if (this.options.reenumerateOnEntry) {
      this.detach()
      this.later(this.options.reenumerateMs, () => this.attach())
    }
    if (download) {
      this.bootDownload('0x15 (USB_UART_CHIP_RESET)')
      if (fromApp) {
        this.setWatchdog(this.options.watchdogMs)
      }
    } else {
      this.bootApp('0x15 (USB_UART_CHIP_RESET)')
    }
  }

  private setWatchdog(ms: number): void {
    if (this.watchdog !== undefined) {
      this.clearLater(this.watchdog)
    }
    this.watchdog = this.later(ms, () => {
      this.watchdog = undefined
      this.watchdogReset()
    })
  }

  private watchdogReset(): void {
    this.resets += 1
    this.stopChip()
    this.detach()
    this.bootApp('0x10 (RTCWDT_RTC_RST)')
    this.later(this.options.reenumerateMs, () => this.attach())
  }

  /** An RTS reset is a system reset; the RTC WDT lives in the RTC domain and keeps running through it. */
  private stopChip(
    options: { readonly keepWatchdog: boolean } = { keepWatchdog: false },
  ): void {
    if (this.watchdog !== undefined && !options.keepWatchdog) {
      this.clearLater(this.watchdog)
      this.watchdog = undefined
    }
    if (this.uptime !== undefined) {
      clearInterval(this.uptime)
      this.uptime = undefined
    }
    this.reading = undefined
    this.writing = undefined
    this.upload = undefined
    this.silent = false
  }

  private boot(): void {
    this.downloadStrap = false
    this.decoder.reset()
    this.consoleLine = ''
    this.outbox = []
    this.pending = []
    this.pendingBytes = 0
    this.uptimeMs = 0
    this.rtcWdtOff = false
    this.swdFed = false
    this.registers = new Map([
      [REG.MAC0, macWords(this.options.mac).mac0],
      [REG.MAC1, macWords(this.options.mac).mac1],
    ])
  }

  private bootApp(cause: string): void {
    this.boot()
    this.mode = 'app'
    this.lastReset = cause
    this.emit(text.encode(appBanner(cause)))
    this.uptime = setInterval(() => {
      this.uptimeMs += 500
      this.emit(text.encode(`{"uptime_ms":${this.uptimeMs}}\r\n`))
    }, 500)
  }

  private bootDownload(cause: string): void {
    this.boot()
    this.mode = 'rom'
    this.lastReset = cause
    this.emit(text.encode(downloadBanner(cause)))
  }

  private attach(): void {
    if (this.mode === 'unplugged') {
      return
    }
    this.enumerations += 1
    this.port = new FakeSerialPort(this)
    const port = this.port
    for (const listener of this.attachListeners.slice()) {
      listener(port)
    }
  }

  private detach(): void {
    const port = this.port
    if (port === undefined) {
      return
    }
    this.port = undefined
    port.lose()
    for (const listener of this.detachListeners.slice()) {
      listener(port)
    }
  }

  // ------------------------------------------------------------------ output

  /**
   * Output leaves in a later macrotask, as USB transfers do; answering
   * inside the host's write would run esptool-js and the sim as one
   * microtask chain that starves every timer on the page.
   */
  private emit(bytes: Uint8Array): void {
    this.outbox.push(bytes)
    if (this.outbox.length === 1) {
      this.later(this.options.replyDelayMs, () => this.flush())
    }
  }

  private flush(): void {
    const port = this.port
    for (const bytes of this.outbox.splice(0)) {
      if (port?.isOpen === true) {
        port.deliver(bytes)
      } else if (this.pendingBytes + bytes.length <= PENDING_LIMIT) {
        this.pending.push(bytes)
        this.pendingBytes += bytes.length
      }
    }
  }

  private later(ms: number, run: () => void): ReturnType<typeof setTimeout> {
    const timer = setTimeout(() => {
      this.timers.delete(timer)
      run()
    }, ms)
    this.timers.add(timer)
    return timer
  }

  private clearLater(timer: ReturnType<typeof setTimeout>): void {
    clearTimeout(timer)
    this.timers.delete(timer)
  }

  private sendFrame(payload: Uint8Array): void {
    this.emit(slipEncode(payload))
  }

  /** A response: direction 1, the op, data length, a value word, data, then status bytes. */
  private reply(
    op: number,
    options: {
      readonly value?: number
      readonly data?: Uint8Array
      readonly error?: number
    } = {},
  ): void {
    const status =
      this.mode === 'rom'
        ? [options.error === undefined ? 0 : 1, options.error ?? 0, 0, 0]
        : [options.error === undefined ? 0 : 1, options.error ?? 0]
    const body = [...(options.data ?? []), ...status]
    this.sendFrame(
      Uint8Array.from([
        0x01,
        op,
        body.length & 0xff,
        (body.length >>> 8) & 0xff,
        ...le32(options.value ?? 0),
        ...body,
      ]),
    )
  }

  // ----------------------------------------------------------------- the app

  private consoleInput(bytes: Uint8Array): void {
    for (const byte of bytes) {
      if (byte === 0x0a) {
        this.consoleCommand(this.consoleLine.trim())
        this.consoleLine = ''
      } else if (byte !== 0x0d && this.consoleLine.length < 256) {
        this.consoleLine += String.fromCharCode(byte)
      }
    }
  }

  private consoleCommand(line: string): void {
    const match = /^CMD (\d+) (\S+)/.exec(line)
    if (match === null) {
      return
    }
    const [, id, verb] = match
    const reply =
      verb === 'INFO'
        ? { status: 'OK', data: 'device=sim-c606' }
        : { status: 'ERR', data: 'unknown command' }
    this.emit(
      text.encode(
        `${JSON.stringify({ type: 'reply', id: Number(id), ...reply })}\r\n`,
      ),
    )
  }

  // ------------------------------------------------------- the loader wire

  private frame(frame: Uint8Array): void {
    if (this.silent) {
      return
    }
    if (this.reading !== undefined) {
      if (frame.length === 4) {
        this.reading.acked = u32(frame, 0)
        this.pump()
      }
      return
    }
    if (frame.length < 8 || frame[0] !== 0x00) {
      return
    }
    const op = frame[1] ?? 0
    const size = (frame[2] ?? 0) | ((frame[3] ?? 0) << 8)
    const check = u32(frame, 4)
    const data = frame.subarray(8, 8 + size)
    this.command(op, check, data)
  }

  private command(op: number, check: number, data: Uint8Array): void {
    const stub = this.mode === 'stub'
    switch (op) {
      case OP.SYNC:
        for (let i = 0; i < (stub ? 1 : 8); i++) {
          this.reply(op, { value: stub ? 0 : 0x20120707 })
        }
        return
      case OP.READ_REG:
        return this.reply(op, {
          value: this.registers.get(u32(data, 0)) ?? 0,
        })
      case OP.WRITE_REG:
        for (let at = 0; at + 16 <= data.length; at += 16) {
          this.writeRegister(
            u32(data, at),
            u32(data, at + 4),
            u32(data, at + 8),
          )
        }
        this.reply(op)
        if (this.armed()) {
          this.silent = true
          this.setWatchdog(WATCHDOG_ARM_MS)
        }
        return
      case OP.GET_SECURITY_INFO:
        return this.reply(op, {
          data: Uint8Array.from([
            ...le32(this.options.secureBoot ? 1 : 0),
            this.options.flashCryptCnt,
            ...Array.from({ length: 7 }, () => 0),
            ...le32(9),
            ...le32(0),
          ]),
        })
      case OP.SPI_FLASH_MD5: {
        const address = u32(data, 0)
        const length = u32(data, 4)
        if (address + length > this.flash.length) {
          return this.reply(op, {
            error: stub ? STUB_BAD_DATA_LEN : ROM_FAILED,
          })
        }
        const digest = this.flashMd5(address, length)
        return this.reply(op, {
          data: stub ? hexBytes(digest) : text.encode(digest),
        })
      }
      case OP.SPI_ATTACH:
      case OP.SPI_SET_PARAMS:
      case OP.CHANGE_BAUDRATE:
        return this.reply(op)
      case OP.MEM_BEGIN:
      case OP.MEM_DATA:
      case OP.MEM_END:
        return stub
          ? this.reply(op, { error: STUB_INVALID_COMMAND })
          : this.memory(op, check, data)
      case OP.FLASH_DEFL_BEGIN:
      case OP.FLASH_DEFL_DATA:
      case OP.FLASH_DEFL_END:
      case OP.READ_FLASH:
      case OP.ERASE_FLASH:
      case OP.ERASE_REGION:
        return stub
          ? this.stubFlash(op, check, data)
          : this.reply(op, { error: ROM_INVALID_MESSAGE })
      default:
        return this.reply(op, {
          error: stub ? STUB_INVALID_COMMAND : ROM_INVALID_MESSAGE,
        })
    }
  }

  private writeRegister(address: number, value: number, mask: number): void {
    const unlocked =
      address === REG.RTC_WDT_CONFIG0 || address === REG.RTC_WDT_CONFIG1
        ? this.registers.get(REG.RTC_WDT_WPROTECT) === RTC_WDT_KEY
        : address === REG.SWD_CONF
          ? this.registers.get(REG.SWD_WPROTECT) === SWD_KEY
          : true
    if (!unlocked) {
      return
    }
    const old = this.registers.get(address) ?? 0
    const next = ((old & ~mask) | (value & mask)) >>> 0
    this.registers.set(address, next)
    if (address === REG.RTC_WDT_CONFIG0) {
      this.rtcWdtOff = (next & ENABLE) === 0
    }
    if (address === REG.SWD_CONF && (next & SWD_AUTO_FEED) !== 0) {
      this.swdFed = true
    }
    if (this.rtcWdtOff && this.swdFed && this.watchdog !== undefined) {
      this.clearLater(this.watchdog)
      this.watchdog = undefined
    }
  }

  private armed(): boolean {
    return ((this.registers.get(REG.RTC_WDT_CONFIG0) ?? 0) & ENABLE) !== 0
  }

  private memory(op: number, check: number, data: Uint8Array): void {
    if (op === OP.MEM_BEGIN) {
      if (this.upload !== undefined && this.upload.seq < this.upload.blocks) {
        return this.reply(op, { error: ROM_FAILED })
      }
      this.upload = {
        size: u32(data, 0),
        blocks: u32(data, 4),
        received: 0,
        seq: 0,
      }
      return this.reply(op)
    }
    const upload = this.upload
    if (op === OP.MEM_DATA) {
      const block = data.subarray(16)
      if (upload === undefined || u32(data, 4) !== upload.seq) {
        return this.reply(op, { error: ROM_FAILED })
      }
      if (checksum(block) !== check || block.length !== u32(data, 0)) {
        return this.reply(op, { error: ROM_BAD_CHECKSUM })
      }
      upload.seq += 1
      upload.received += block.length
      return this.reply(op)
    }
    if (upload === undefined || upload.received !== upload.size) {
      return this.reply(op, { error: ROM_FAILED })
    }
    this.upload = undefined
    this.reply(op)
    if (u32(data, 0) === 0) {
      this.mode = 'stub'
      this.sendFrame(text.encode('OHAI'))
    }
  }

  private stubFlash(op: number, check: number, data: Uint8Array): void {
    switch (op) {
      case OP.FLASH_DEFL_BEGIN: {
        const size = u32(data, 0)
        const address = u32(data, 12)
        if (address + size > this.flash.length) {
          return this.reply(op, { error: STUB_BAD_DATA_LEN })
        }
        let written = 0
        const inflate = new Inflate()
        inflate.onData = chunk => {
          const bytes =
            chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk)
          this.flash.set(
            bytes.subarray(0, Math.max(0, size - written)),
            address + written,
          )
          written += bytes.length
        }
        this.writing = { inflate, blocks: u32(data, 4), seq: 0 }
        return this.reply(op)
      }
      case OP.FLASH_DEFL_DATA: {
        const writing = this.writing
        const block = data.subarray(16)
        if (writing === undefined || u32(data, 4) !== writing.seq) {
          return this.reply(op, { error: STUB_BAD_DATA_LEN })
        }
        if (checksum(block) !== check || block.length !== u32(data, 0)) {
          return this.reply(op, { error: STUB_BAD_CHECKSUM })
        }
        writing.inflate.push(block, false)
        if (writing.inflate.err !== 0) {
          return this.reply(op, { error: STUB_INFLATE_ERROR })
        }
        writing.seq += 1
        return this.reply(op)
      }
      case OP.FLASH_DEFL_END:
        this.writing = undefined
        return this.reply(op)
      case OP.ERASE_FLASH:
        this.flash.fill(0xff)
        return this.reply(op)
      case OP.ERASE_REGION: {
        const address = u32(data, 0)
        this.flash.fill(0xff, address, address + u32(data, 4))
        return this.reply(op)
      }
      case OP.READ_FLASH:
        return this.readFlash(data)
    }
  }

  private readFlash(data: Uint8Array): void {
    const address = u32(data, 0)
    const size = u32(data, 4)
    if (address + size > this.flash.length) {
      return this.reply(OP.READ_FLASH, { error: STUB_BAD_DATA_LEN })
    }
    const fault = this.faults.find(
      f =>
        f.times > 0 &&
        f.address < address + size &&
        address < f.address + f.size,
    )
    if (fault !== undefined) {
      fault.times -= 1
    }
    this.reply(OP.READ_FLASH)
    this.reading = {
      address,
      size,
      packet: u32(data, 8),
      window: u32(data, 12),
      corruptAt:
        fault === undefined ? undefined : Math.max(fault.address, address),
      sent: 0,
      acked: 0,
    }
    this.pump()
  }

  /** The stub keeps `window` packets unacknowledged, then sends the MD5 of the range once all are acked. */
  private pump(): void {
    const reading = this.reading
    if (reading === undefined) {
      return
    }
    while (
      reading.sent < reading.size &&
      reading.sent < reading.acked + reading.window * reading.packet
    ) {
      const from = reading.address + reading.sent
      const length = Math.min(reading.packet, reading.size - reading.sent)
      const packet = this.flash.slice(from, from + length)
      const corrupt = reading.corruptAt
      if (corrupt !== undefined && corrupt >= from && corrupt < from + length) {
        packet[corrupt - from] = (packet[corrupt - from] ?? 0) ^ 0x01
      }
      this.sendFrame(packet)
      reading.sent += length
    }
    if (reading.acked >= reading.size) {
      this.reading = undefined
      this.sendFrame(hexBytes(this.flashMd5(reading.address, reading.size)))
    }
  }
}
