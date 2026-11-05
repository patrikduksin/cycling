import * as Config from 'effect/Config'
import * as Context from 'effect/Context'
import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import type * as Redacted from 'effect/Redacted'
import * as Schema from 'effect/Schema'

import { RELAY_NOT_TLS, TOKEN_TOO_SHORT } from './Status.ts'

/**
 * `wss://relay.cycling.computer`: a scheme and a host, no path; the agent
 * appends `/v2/agent/<code>`. Plain `ws://` would send the token in the
 * clear, so it is only for a relay on this machine.
 */
export const RelayUrl = Schema.String.check(
  Schema.isPattern(
    /^(wss:\/\/[^/]+|ws:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?)$/,
    { message: RELAY_NOT_TLS },
  ),
)

/** The relay's AGENT_TOKEN; a short one (or none) is a typo or a placeholder, not a secret. */
export const AgentToken = Schema.RedactedFromValue(
  Schema.String.check(Schema.isMinLength(32, { message: TOKEN_TOO_SHORT })),
)

/**
 * Everything the Python launcher hands over, as env. The bun side never
 * learns the repo layout: `dir` is the parent of the session directories.
 */
export class AgentConfig extends Context.Service<
  AgentConfig,
  {
    readonly relay: string
    readonly token: Redacted.Redacted<string>
    readonly dir: string
  }
>()('agent/AgentConfig') {}

export const fromEnv: Layer.Layer<AgentConfig, Config.ConfigError> =
  Layer.effect(
    AgentConfig,
    Effect.gen(function* () {
      const token = yield* Config.String('CYCLING_REMOTE_TOKEN').pipe(
        Config.withDefault(''),
        Config.mapEffect(raw =>
          Schema.decodeEffect(AgentToken)(raw).pipe(
            Effect.mapError(error => new Config.ConfigError(error)),
          ),
        ),
      )
      const dir = yield* Config.NonEmptyString('CYCLING_REMOTE_DIR')
      const relay = yield* Config.schema(RelayUrl, 'CYCLING_REMOTE_RELAY').pipe(
        Config.withDefault('wss://relay.cycling.computer'),
      )
      return AgentConfig.of({ relay, token, dir })
    }),
  )
