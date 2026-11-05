import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Fiber from 'effect/Fiber'
import * as Stream from 'effect/Stream'

import {
  ReacquireTimeout,
  SerialLinkLost,
  SerialOpenRefused,
} from '../links.ts'
import { baud, identity, otherIdentity } from '../test/fixtures.ts'
import { makeWebSerial } from './serial.ts'

interface Call {
  readonly name: string
  readonly detail?: unknown
}

/** A port as Chrome exposes it: ids, open/close, two streams and signals. */
class FakePort {
  readable: ReadableStream<Uint8Array> | null = null
  writable: WritableStream<Uint8Array> | null = null
  readonly calls: Array<Call> = []
  readonly written: Array<ReadonlyArray<number>> = []
  private incoming: ReadableStreamDefaultController<Uint8Array> | undefined

  constructor(
    private readonly info: { usbVendorId: number; usbProductId: number },
  ) {}

  getInfo() {
    return this.info
  }

  open(options: unknown): Promise<void> {
    this.calls.push({ name: 'open', detail: options })
    this.readable = new ReadableStream<Uint8Array>({
      start: controller => {
        this.incoming = controller
      },
    })
    this.writable = new WritableStream<Uint8Array>({
      write: chunk => {
        this.written.push(Array.from(chunk))
      },
    })
    return Promise.resolve()
  }

  close(): Promise<void> {
    this.calls.push({ name: 'close' })
    this.readable = null
    this.writable = null
    return Promise.resolve()
  }

  setSignals(signals: unknown): Promise<void> {
    this.calls.push({ name: 'setSignals', detail: signals })
    return Promise.resolve()
  }

  receive(...bytes: Array<number>): void {
    this.incoming?.enqueue(Uint8Array.from(bytes))
  }

  unplug(): void {
    this.incoming?.error(new DOMException('lost', 'NetworkError'))
  }
}

const makePort = (info: { usbVendorId: number; usbProductId: number }) =>
  new FakePort(info)

const listeners = new Set<() => void>()
let granted: Array<FakePort> = []
let chosen: FakePort | undefined
let serial = makeWebSerial()
const originalSerial = Object.getOwnPropertyDescriptor(navigator, 'serial')

beforeEach(() => {
  granted = []
  chosen = undefined
  serial = makeWebSerial()
  listeners.clear()
  Object.defineProperty(navigator, 'serial', {
    configurable: true,
    value: {
      getPorts: () => Promise.resolve(granted),
      requestPort: () =>
        chosen === undefined
          ? Promise.reject(new DOMException('none', 'NotFoundError'))
          : Promise.resolve(chosen),
      addEventListener: (_type: string, listener: () => void) => {
        listeners.add(listener)
      },
      removeEventListener: (_type: string, listener: () => void) => {
        listeners.delete(listener)
      },
    },
  })
})

afterEach(() => {
  if (originalSerial === undefined) {
    Reflect.deleteProperty(navigator, 'serial')
  } else {
    Object.defineProperty(navigator, 'serial', originalSerial)
  }
})

const c606 = {
  usbVendorId: identity.vendorId,
  usbProductId: identity.productId,
}
const acquire = (waitMs: number) =>
  serial.acquire({ identity, baud: baud(115_200), waitMs })

describe('serial adapter acquire', () => {
  test('opens the granted port with the baud and a roomy receive buffer', () =>
    Effect.runPromise(
      Effect.gen(function* () {
        const device = makePort(c606)
        granted = [device]
        yield* acquire(0)
        expect(device.calls[0]).toEqual({
          name: 'open',
          detail: { baudRate: 115_200, bufferSize: 65_536 },
        })
      }).pipe(Effect.scoped),
    ))

  test('skips ports with another identity', () =>
    Effect.runPromise(
      Effect.gen(function* () {
        const other = makePort({
          usbVendorId: otherIdentity.vendorId,
          usbProductId: otherIdentity.productId,
        })
        const wanted = makePort(c606)
        granted = [other, wanted]
        yield* acquire(0)
        expect(other.calls).toEqual([])
        expect(wanted.calls.map(call => call.name)).toEqual(['open'])
      }).pipe(Effect.scoped),
    ))

  test('with no wait, a missing port is refused at once', async () => {
    const exit = await Effect.runPromiseExit(acquire(0).pipe(Effect.scoped))
    expect(Exit.isFailure(exit) && String(exit.cause)).toContain(
      new SerialOpenRefused({ reason: '' })._tag,
    )
  })

  test('waits for the port to come back and wakes on the connect event', () =>
    Effect.runPromise(
      Effect.gen(function* () {
        const device = makePort(c606)
        const started = Date.now()
        const opening = yield* Effect.forkChild(acquire(5_000))
        yield* Effect.sleep('40 millis')
        granted = [device]
        listeners.forEach(listener => listener())
        yield* Fiber.join(opening)
        expect(device.calls.map(call => call.name)).toEqual(['open'])
        expect(Date.now() - started).toBeLessThan(200)
      }).pipe(Effect.scoped),
    ))

  test('gives up with ReacquireTimeout when the port never returns', async () => {
    const exit = await Effect.runPromiseExit(acquire(300).pipe(Effect.scoped))
    expect(Exit.isFailure(exit) && String(exit.cause)).toContain(
      new ReacquireTimeout()._tag,
    )
  })
})

describe('serial adapter keeps the port the visitor picked', () => {
  const opens = (port: FakePort) =>
    port.calls.filter(call => call.name === 'open').length

  test('opens the picked port, not another board with the same ids', () =>
    Effect.runPromise(
      Effect.gen(function* () {
        const twin = makePort(c606)
        const picked = makePort(c606)
        granted = [twin, picked]
        chosen = picked
        expect(yield* serial.choose).toEqual(identity)
        yield* acquire(0)
        expect(opens(picked)).toBe(1)
        expect(opens(twin)).toBe(0)
      }).pipe(Effect.scoped),
    ))

  test('after a USB reset, the picked port is preferred when it comes back', () =>
    Effect.runPromise(
      Effect.gen(function* () {
        const picked = makePort(c606)
        const other = makePort(c606)
        granted = [picked]
        chosen = picked
        yield* serial.choose
        yield* acquire(0).pipe(Effect.scoped)

        granted = [other, picked]
        yield* acquire(5_000)
        expect(opens(picked)).toBe(2)
        expect(opens(other)).toBe(0)
      }).pipe(Effect.scoped),
    ))

  test('when the picked port is gone, the first matching port nobody has open is used', () =>
    Effect.runPromise(
      Effect.gen(function* () {
        const picked = makePort(c606)
        const busy = makePort(c606)
        const free = makePort(c606)
        granted = [picked]
        chosen = picked
        yield* serial.choose
        yield* acquire(0).pipe(Effect.scoped)
        yield* Effect.promise(() => busy.open({}))

        granted = [busy, free]
        yield* acquire(5_000)
        expect(opens(free)).toBe(1)
        expect(opens(busy)).toBe(1)
      }).pipe(Effect.scoped),
    ))
})

describe('serial link', () => {
  test('streams what the device sends and fails with SerialLinkLost when it is unplugged', async () => {
    const device = makePort(c606)
    granted = [device]
    const outcome = await Effect.runPromise(
      Effect.gen(function* () {
        const link = yield* acquire(0)
        const reading = yield* Effect.forkChild(
          Stream.runCollect(link.input).pipe(Effect.flip),
        )
        device.receive(1, 2)
        yield* Effect.sleep('10 millis')
        device.unplug()
        return yield* Fiber.await(reading)
      }).pipe(Effect.scoped),
    )
    expect(Exit.isSuccess(outcome) && outcome.value).toBeInstanceOf(
      SerialLinkLost,
    )
  })

  test('writes bytes to the port', () =>
    Effect.runPromise(
      Effect.gen(function* () {
        const device = makePort(c606)
        granted = [device]
        const link = yield* acquire(0)
        yield* link.write(Uint8Array.of(0, 255, 17, 19))
        expect(device.written).toEqual([[0, 255, 17, 19]])
      }).pipe(Effect.scoped),
    ))

  test('closing the scope closes the port', async () => {
    const device = makePort(c606)
    granted = [device]
    await Effect.runPromise(acquire(0).pipe(Effect.scoped))
    expect(device.calls.map(call => call.name)).toEqual(['open', 'close'])
  })
})
