import { describe, expect, test } from 'bun:test'
import * as ConfigProvider from 'effect/ConfigProvider'
import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import * as Redacted from 'effect/Redacted'

import { AgentConfig, fromEnv } from '../src/AgentConfig.ts'
import { RELAY_NOT_TLS, TOKEN_TOO_SHORT } from '../src/Status.ts'

const TOKEN = 'k'.repeat(32)

const load = (env: Record<string, string>) =>
  Effect.runPromise(
    Effect.service(AgentConfig).pipe(
      Effect.provide(
        fromEnv.pipe(
          Layer.provide(
            ConfigProvider.layer(
              ConfigProvider.fromEnvRecord({
                CYCLING_REMOTE_TOKEN: TOKEN,
                CYCLING_REMOTE_DIR: '/tmp/remote',
                ...env,
              }),
            ),
          ),
        ),
      ),
      Effect.map(config => ({
        relay: config.relay,
        token: Redacted.value(config.token),
      })),
      Effect.catchTag('ConfigError', error => Effect.succeed(error.message)),
    ),
  )

describe('agent config (L6, L7)', () => {
  test('a 32-character token is accepted and the relay defaults to wss://relay.cycling.computer', async () => {
    expect(await load({})).toEqual({
      relay: 'wss://relay.cycling.computer',
      token: TOKEN,
    })
  })

  for (const token of ['', 'smoke-agent-token', 'k'.repeat(31)]) {
    test(`a token of ${token.length} characters is refused`, async () => {
      expect(await load({ CYCLING_REMOTE_TOKEN: token })).toContain(
        TOKEN_TOO_SHORT,
      )
    })
  }

  for (const relay of [
    'wss://relay.cycling.computer',
    'wss://relay.example:8443',
    'ws://localhost:8787',
    'ws://127.0.0.1:51234',
    'ws://[::1]:8787',
    'ws://localhost',
  ]) {
    test(`relay ${relay} is accepted`, async () => {
      expect(await load({ CYCLING_REMOTE_RELAY: relay })).toEqual({
        relay,
        token: TOKEN,
      })
    })
  }

  for (const relay of [
    'ws://relay.cycling.computer',
    'ws://192.168.1.20:8787',
    'ws://localhost.evil.example',
    'ws://127.0.0.1.nip.io',
    'http://localhost:8787',
    'wss://relay.cycling.computer/v1',
  ]) {
    test(`relay ${relay} is refused`, async () => {
      expect(await load({ CYCLING_REMOTE_RELAY: relay })).toContain(
        RELAY_NOT_TLS,
      )
    })
  }
})
