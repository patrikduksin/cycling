import * as Schema from 'effect/Schema'

import type { LineCommand } from '../../src/ResetRecognizer.ts'
import type { ClientRequest } from '../../src/rfc2217/ComPort.ts'

export type Control = LineCommand | { readonly purge: true }

/** Fixture tokens: D=DTR, R=RTS, 1=on, 0=off, P=purge. */
export const tokenOf = (request: ClientRequest): string | null => {
  if (request._tag === 'Purge') return 'P'
  if (request._tag !== 'Line') return null
  return `${request.command.line === 'dtr' ? 'D' : 'R'}${request.command.on ? 1 : 0}`
}

export const parseControls = (text: string): ReadonlyArray<Control> =>
  text
    .split(/\s+/)
    .filter(token => token.length > 0)
    .map((token): Control => {
      if (token === 'P') return { purge: true }
      const line = token[0] === 'D' ? 'dtr' : 'rts'
      return { line, on: token[1] === '1' }
    })

export const Fixture = Schema.Struct({
  description: Schema.String,
  command: Schema.Array(Schema.String),
  tools: Schema.String,
  controls: Schema.String,
})
export type Fixture = typeof Fixture.Type

export const fixturePath = (name: string) =>
  `${import.meta.dir}/../fixtures/${name}.json`
