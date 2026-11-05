import * as Schema from 'effect/Schema'

/**
 * Resets the browser performs locally with `SerialPort.setSignals`. Single
 * DTR/RTS edges never cross the wire: a ~100 ms round trip would wreck the
 * timing the chip needs.
 */
export const ResetKind = Schema.Literals(['usbJtagBootloader', 'hardReset'])
export type ResetKind = typeof ResetKind.Type

/** One `setSignals` call, then a hold. A line left out keeps its level. Booleans follow pyserial and Web Serial: true asserts. */
export interface SignalStep {
  readonly dataTerminalReady?: boolean
  readonly requestToSend?: boolean
  readonly holdMs: number
}

/**
 * The one copy of the reset timings. The browser executes these steps; the
 * agent's recognizer derives what it matches from them (`recognitionOf`).
 * Transcribed from esp_pylib serial_reset.py (`usb_jtag_bootloader_reset`,
 * `hard_reset`) and checked against the levels esptool sends over RFC 2217
 * (web/agent/test/fixtures). The (true, true) step with no hold is the level
 * esptool deliberately walks through between "IO0" and "reset".
 */
export const RESET_PLANS: Readonly<
  Record<ResetKind, ReadonlyArray<SignalStep>>
> = {
  usbJtagBootloader: [
    { dataTerminalReady: false, requestToSend: false, holdMs: 100 },
    { dataTerminalReady: true, requestToSend: false, holdMs: 100 },
    { dataTerminalReady: true, requestToSend: true, holdMs: 0 },
    { dataTerminalReady: false, requestToSend: true, holdMs: 100 },
    { dataTerminalReady: false, requestToSend: false, holdMs: 0 },
  ],
  hardReset: [
    { requestToSend: true, holdMs: 100 },
    { requestToSend: false, holdMs: 0 },
  ],
}

export type Levels = readonly [dtr: boolean, rts: boolean]

/**
 * How an RFC 2217 client's line changes identify a kind, derived from its plan:
 * a plan that sets both lines on every step is a level sequence; a plan that
 * only moves RTS is an RTS pulse with DTR held.
 */
export type Recognition =
  | { readonly _tag: 'Levels'; readonly levels: ReadonlyArray<Levels> }
  | { readonly _tag: 'RtsPulse' }

export const recognitionOf = (kind: ResetKind): Recognition => {
  const steps = RESET_PLANS[kind]
  const levels: Array<Levels> = []
  for (const { dataTerminalReady, requestToSend } of steps) {
    if (dataTerminalReady === undefined || requestToSend === undefined) {
      return { _tag: 'RtsPulse' }
    }
    levels.push([dataTerminalReady, requestToSend])
  }
  return { _tag: 'Levels', levels }
}
