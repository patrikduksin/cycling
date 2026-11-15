import { afterEach, setDefaultTimeout } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Stream from 'effect/Stream'

import { FlasherError, open, type Session } from '../src/index.ts'
import { SimDevice, type SimOptions } from './sim/device.ts'
import type { FakeSerialPort } from './sim/port.ts'

export const MAC = '02:00:00:12:34:56'

/** A short RTC WDT, so the watchdog cases take about a second of wall clock. */
export const WATCHDOG_MS = 500

export const simulator = (): ((options?: SimOptions) => SimDevice) => {
  setDefaultTimeout(30_000)
  const devices: Array<SimDevice> = []
  afterEach(() => {
    for (const device of devices.splice(0)) {
      device.dispose()
    }
  })
  return (options = {}) => {
    const device = new SimDevice({ mac: MAC, ...options })
    devices.push(device)
    return device
  }
}

/** Deterministic noise, so a dropped or shifted byte cannot go unnoticed. */
export const noise = (length: number, seed: number): Uint8Array => {
  const words = new Uint32Array(Math.ceil(length / 4))
  let state = seed >>> 0 || 1
  for (let i = 0; i < words.length; i++) {
    state ^= state << 13
    state ^= state >>> 17
    state ^= state << 5
    words[i] = state >>> 0
  }
  return new Uint8Array(words.buffer, 0, length)
}

export const portOf = (device: SimDevice): FakeSerialPort => {
  const port = device.current()
  if (port === undefined) {
    throw new Error('the sim has no port enumerated')
  }
  return port
}

export const session = <A, E>(
  device: SimDevice,
  use: (session: Session) => Effect.Effect<A, E>,
): Promise<Exit.Exit<A, E | FlasherError>> =>
  Effect.runPromiseExit(
    Effect.scoped(
      open(portOf(device), { entry: 'reset' }).pipe(Effect.flatMap(use)),
    ),
  )

export const readAll = (
  session: Session,
  address: number,
  size: number,
): Effect.Effect<Uint8Array, FlasherError> =>
  Effect.gen(function* () {
    const assembled = new Uint8Array(size)
    yield* Stream.runForEach(session.read({ address, size }), event =>
      Effect.sync(() => {
        if (event._tag === 'Piece') {
          assembled.set(event.bytes, event.offset)
        }
      }),
    )
    return assembled
  })

export const reasonOf = (
  exit: Exit.Exit<unknown, unknown>,
): string | undefined =>
  Exit.isFailure(exit) && exit.cause.reasons[0]?._tag === 'Fail'
    ? exit.cause.reasons[0].error instanceof FlasherError
      ? exit.cause.reasons[0].error.reason
      : 'not a FlasherError'
    : undefined

export const valueOf = <A>(exit: Exit.Exit<A, unknown>): A => {
  if (Exit.isSuccess(exit)) {
    return exit.value
  }
  throw new Error(`the session failed: ${String(exit.cause)}`)
}

/** Reads what the port prints until `until` matches or `ms` passes. */
export const consoleText = async (
  port: SerialPort,
  until: RegExp,
  ms: number,
): Promise<string> => {
  await port.open({ baudRate: 115200 })
  const readable = port.readable
  if (readable === null) {
    throw new Error('the reopened port has no readable')
  }
  const reader = readable.getReader()
  const decoder = new TextDecoder()
  let text = ''
  let timer: ReturnType<typeof setTimeout> | undefined
  const deadline = new Promise<undefined>(resolve => {
    timer = setTimeout(() => resolve(undefined), ms)
  })
  while (!until.test(text)) {
    const chunk = await Promise.race([reader.read(), deadline])
    if (chunk === undefined || chunk.done) {
      break
    }
    text += decoder.decode(chunk.value)
  }
  clearTimeout(timer)
  await reader.cancel()
  reader.releaseLock()
  await port.close()
  return text
}
