import {
  type Levels,
  recognitionOf,
  type ResetKind,
  ResetKind as ResetKindSchema,
} from '@cycling/protocol'

/** One RFC 2217 SET_CONTROL line command; `on` follows pyserial (true asserts). */
export interface LineCommand {
  readonly line: 'dtr' | 'rts'
  readonly on: boolean
}

export interface Recognizer {
  readonly dtr: boolean | null
  readonly rts: boolean | null
  /** Distinct consecutive (dtr, rts) levels since both lines were first set or a reset last fired. */
  readonly history: ReadonlyArray<Levels>
  readonly armed: boolean
}

export const initialRecognizer: Recognizer = {
  dtr: null,
  rts: null,
  history: [],
  armed: false,
}

const LEVEL_PATTERNS: ReadonlyArray<{
  readonly kind: ResetKind
  readonly levels: ReadonlyArray<Levels>
}> = ResetKindSchema.literals
  .flatMap(kind => {
    const recognition = recognitionOf(kind)
    return recognition._tag === 'Levels'
      ? [{ kind, levels: recognition.levels }]
      : []
  })
  .sort((a, b) => b.levels.length - a.levels.length)

const PULSE_KINDS: ReadonlyArray<ResetKind> = ResetKindSchema.literals.filter(
  kind => recognitionOf(kind)._tag === 'RtsPulse',
)

const sameLevels = (a: Levels, b: Levels) => a[0] === b[0] && a[1] === b[1]

const endsWith = (
  history: ReadonlyArray<Levels>,
  pattern: ReadonlyArray<Levels>,
) => {
  if (pattern.length > history.length) return false
  const offset = history.length - pattern.length
  return pattern.every((levels, index) => {
    const seen = history[offset + index]
    return seen !== undefined && sameLevels(seen, levels)
  })
}

/**
 * Pure. esptool never says "reset"; it walks DTR/RTS over RFC 2217 and the
 * order of levels identifies the kind. No timers: local TCP timing is not
 * the contract, the level order is. A firing restarts the history at the
 * current levels rather than empty: the bootloader plan starts at the levels
 * it ends on, so a back-to-back reset still has its first level.
 */
export const step = (
  state: Recognizer,
  command: LineCommand,
): readonly [Recognizer, ResetKind | null] => {
  const previous = state[command.line]
  const lines = { ...state, [command.line]: command.on }
  const { dtr, rts } = lines
  if (dtr === null || rts === null) {
    return [lines, null]
  }
  const levels: Levels = [dtr, rts]
  if (previous === null) {
    return [{ ...lines, history: [levels] }, null]
  }
  const last = state.history.at(-1)
  const history =
    last !== undefined && sameLevels(last, levels)
      ? state.history
      : [...state.history, levels]
  const fire = (kind: ResetKind): readonly [Recognizer, ResetKind] => [
    { dtr, rts, history: [levels], armed: false },
    kind,
  ]
  const matched = LEVEL_PATTERNS.find(pattern =>
    endsWith(history, pattern.levels),
  )
  if (matched !== undefined) {
    return fire(matched.kind)
  }
  const pulse = PULSE_KINDS[0]
  if (command.line === 'rts') {
    if (command.on) {
      return [{ ...lines, history, armed: true }, null]
    }
    if (state.armed && pulse !== undefined) {
      return fire(pulse)
    }
    return [{ ...lines, history, armed: false }, null]
  }
  return [
    { ...lines, history, armed: state.armed && previous === command.on },
    null,
  ]
}
