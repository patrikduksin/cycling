/** What a FakeSerialPort needs from the device behind it. */
export interface Wire {
  readonly opened: (port: FakeSerialPort) => void
  readonly closed: (port: FakeSerialPort) => void
  readonly receive: (port: FakeSerialPort, bytes: Uint8Array) => void
  readonly signals: (signals: SerialOutputSignals) => void
}

const ESPRESSIF_USB_JTAG = { usbVendorId: 0x303a, usbProductId: 0x1001 }
const CHROME_BUFFER_SIZE = 255

type PortState = 'closed' | 'open' | 'lost'

/**
 * One USB enumeration of the simulated chip, as Chrome's Web Serial exposes
 * it. A reset that drops the chip off the bus loses this object for good;
 * the next enumeration is a new FakeSerialPort.
 */
export class FakeSerialPort extends EventTarget implements SerialPort {
  onconnect: ((this: SerialPort, event: Event) => unknown) | null = null
  ondisconnect: ((this: SerialPort, event: Event) => unknown) | null = null
  readable: ReadableStream<Uint8Array> | null = null
  writable: WritableStream<Uint8Array> | null = null
  private state: PortState = 'closed'
  private incoming: ReadableStreamDefaultController<Uint8Array> | undefined
  /** Chrome hands the page at most `bufferSize` bytes per read, 255 unless the opener asked for more. */
  private readSize = CHROME_BUFFER_SIZE

  constructor(private readonly wire: Wire) {
    super()
  }

  get connected(): boolean {
    return this.state !== 'lost'
  }

  get isOpen(): boolean {
    return this.state === 'open'
  }

  getInfo(): SerialPortInfo {
    return ESPRESSIF_USB_JTAG
  }

  open(options: SerialOptions): Promise<void> {
    if (this.state === 'lost') {
      return Promise.reject(
        new DOMException('The device has been lost.', 'NetworkError'),
      )
    }
    if (this.state === 'open') {
      return Promise.reject(
        new DOMException('The port is already open.', 'InvalidStateError'),
      )
    }
    this.state = 'open'
    this.readSize = options.bufferSize ?? CHROME_BUFFER_SIZE
    this.readable = new ReadableStream<Uint8Array>({
      start: controller => {
        this.incoming = controller
      },
      cancel: () => {
        this.readable = null
        this.incoming = undefined
      },
    })
    this.writable = new WritableStream<Uint8Array>({
      write: chunk => this.wire.receive(this, Uint8Array.from(chunk)),
    })
    this.wire.opened(this)
    return Promise.resolve()
  }

  close(): Promise<void> {
    if (this.readable?.locked === true || this.writable?.locked === true) {
      return Promise.reject(
        new DOMException('A stream is locked.', 'InvalidStateError'),
      )
    }
    if (this.state === 'open') {
      this.state = 'closed'
      this.incoming?.close()
      this.incoming = undefined
      this.readable = null
      this.writable = null
      this.wire.closed(this)
    }
    return Promise.resolve()
  }

  setSignals(signals: SerialOutputSignals = {}): Promise<void> {
    if (this.state !== 'open') {
      return Promise.reject(
        new DOMException('The port is not open.', 'InvalidStateError'),
      )
    }
    this.wire.signals(signals)
    return Promise.resolve()
  }

  getSignals(): Promise<SerialInputSignals> {
    return Promise.resolve({
      clearToSend: false,
      dataCarrierDetect: false,
      dataSetReady: false,
      ringIndicator: false,
    })
  }

  forget(): Promise<void> {
    return Promise.resolve()
  }

  /** Device output; dropped unless the host has the port open. */
  deliver(bytes: Uint8Array): void {
    for (let at = 0; at < bytes.length; at += this.readSize) {
      this.incoming?.enqueue(bytes.slice(at, at + this.readSize))
    }
  }

  /** The device left the bus: reads fail, the streams go away, `disconnect` fires. */
  lose(): void {
    if (this.state === 'lost') {
      return
    }
    this.state = 'lost'
    this.incoming?.error(
      new DOMException('The device has been lost.', 'NetworkError'),
    )
    this.incoming = undefined
    this.readable = null
    this.writable = null
    this.dispatchEvent(new Event('disconnect'))
  }
}

export interface Enumeration {
  readonly onAttach: (listener: (port: FakeSerialPort) => void) => void
  readonly onDetach: (listener: (port: FakeSerialPort) => void) => void
  readonly current: () => FakeSerialPort | undefined
}

/**
 * `navigator.serial` with one simulated device. The visitor's grant survives
 * re-enumeration, as Chrome keeps it per USB device; `connect` and
 * `disconnect` fire here as well as on the port.
 */
export class FakeSerial extends EventTarget implements Serial {
  onconnect: ((this: Serial, event: Event) => unknown) | null = null
  ondisconnect: ((this: Serial, event: Event) => unknown) | null = null
  private granted = false

  constructor(private readonly device: Enumeration) {
    super()
    device.onAttach(() => this.dispatchEvent(new Event('connect')))
    device.onDetach(() => this.dispatchEvent(new Event('disconnect')))
  }

  getPorts(): Promise<Array<SerialPort>> {
    const port = this.device.current()
    return Promise.resolve(this.granted && port !== undefined ? [port] : [])
  }

  requestPort(_options?: SerialPortRequestOptions): Promise<SerialPort> {
    const port = this.device.current()
    if (port === undefined) {
      return Promise.reject(
        new DOMException('No port selected by the user.', 'NotFoundError'),
      )
    }
    this.granted = true
    return Promise.resolve(port)
  }
}
