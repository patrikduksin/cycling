import { describe, expect, test } from 'bun:test'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'

import {
  agentProtocols,
  browserProtocols,
  CloseCode,
  mintCode,
  mintOwnerKey,
  parseSessionPath,
  type Role,
  sessionPath,
} from '@cycling/protocol'

import {
  type Admission,
  admit,
  originAllowed,
  type OriginPolicy,
  parseAgentToken,
  parseOrigins,
  secretsEqual,
  sha256Hex,
} from './admission.ts'

const code = mintCode(new Uint8Array([1, 2, 3, 4, 5]))
const key = mintOwnerKey(new Uint8Array(16).fill(9))
const production: OriginPolicy = {
  allowed: ['https://cycling.computer'],
  allowLocal: false,
}
const development: OriginPolicy = { ...production, allowLocal: true }
const TOKEN = 's3cret-'.repeat(5)
const credentials = (
  origins: OriginPolicy,
  agentToken: Option.Option<string> = Option.some(TOKEN),
) => ({
  agentToken: parseAgentToken(agentToken),
  origins,
})

describe('originAllowed', () => {
  test.each([
    ['https://cycling.computer', production, true],
    ['https://evil.example', production, false],
    ['http://localhost:5173', production, false],
    ['http://localhost:5173', development, true],
    ['http://127.0.0.1:8787', development, true],
    ['http://localhost.evil.example', development, false],
    ['https://localhost:5173', development, false],
    [undefined, development, false],
  ] as const)('%s', (origin, policy, expected) => {
    expect(originAllowed(origin, policy)).toBe(expected)
  })

  test('parses a comma separated list', () => {
    expect(parseOrigins(' https://a.example, ,https://b.example ')).toEqual([
      'https://a.example',
      'https://b.example',
    ])
  })
})

describe('parseAgentToken', () => {
  test.each([
    [Option.none(), false],
    [Option.some(''), false],
    [Option.some('x'.repeat(31)), false],
    [Option.some('x'.repeat(32)), true],
    [Option.some('x'.repeat(64)), true],
  ] as const)('%o is usable: %s', (secret, usable) => {
    expect(Option.isSome(parseAgentToken(secret))).toBe(usable)
  })
})

describe('secretsEqual', () => {
  test.each([
    ['same', 'same', true],
    ['same', 'sama', false],
    ['same', 'sam', false],
    ['', 'x', false],
  ] as const)('%s vs %s', async (left, right, expected) => {
    expect(await Effect.runPromise(secretsEqual(left, right))).toBe(expected)
  })
})

describe('admit', () => {
  const route = (role: Role, pathname = sessionPath(role, code)) =>
    Option.getOrThrow(parseSessionPath(pathname))
  const run = (
    role: Role,
    protocols: ReadonlyArray<string> | null,
    origin: string | undefined,
    options: {
      pathname?: string
      origins?: OriginPolicy
      agentToken?: Option.Option<string>
    } = {},
  ): Promise<Admission> =>
    Effect.runPromise(
      admit(
        {
          route: route(role, options.pathname),
          protocolHeader: protocols === null ? null : protocols.join(', '),
          origin,
        },
        credentials(options.origins ?? production, options.agentToken),
      ),
    )

  const refusal = (close: number) =>
    expect.objectContaining({ _tag: 'Refuse', close })

  test('admits a browser with its owner key from an allowed origin and hashes the key', async () => {
    const result = await run(
      'browser',
      browserProtocols(key),
      'https://cycling.computer',
    )
    expect(result).toEqual({
      _tag: 'Admit',
      role: 'browser',
      code,
      ownerKeyHash: await Effect.runPromise(sha256Hex(key)),
    })
  })

  test('admits an agent with the right token and no origin', async () => {
    expect(await run('agent', agentProtocols(TOKEN), undefined)).toEqual({
      _tag: 'Admit',
      role: 'agent',
      code,
    })
  })

  test('refuses a bad token with unauthorized', async () => {
    expect(await run('agent', agentProtocols('nope'), undefined)).toEqual(
      refusal(CloseCode.unauthorized),
    )
  })

  test('refuses an agent with no token', async () => {
    expect(await run('agent', ['cycling.v2'], undefined)).toEqual(
      refusal(CloseCode.unauthorized),
    )
  })

  test('refuses a browser with no owner key', async () => {
    expect(
      await run('browser', ['cycling.v2'], 'https://cycling.computer'),
    ).toEqual(refusal(CloseCode.unauthorized))
  })

  test('refuses a browser from a foreign or missing origin', async () => {
    expect(
      await run('browser', browserProtocols(key), 'https://evil.example'),
    ).toEqual(refusal(CloseCode.unauthorized))
    expect(await run('browser', browserProtocols(key), undefined)).toEqual(
      refusal(CloseCode.unauthorized),
    )
  })

  test('allows localhost only where the policy says so', async () => {
    expect(
      (await run('browser', browserProtocols(key), 'http://localhost:5173'))
        ._tag,
    ).toBe('Refuse')
    expect(
      (
        await run('browser', browserProtocols(key), 'http://localhost:5173', {
          origins: development,
        })
      )._tag,
    ).toBe('Admit')
  })

  test('refuses a missing version token before anything else', async () => {
    expect(await run('agent', null, undefined)).toEqual(
      refusal(CloseCode.unsupportedVersion),
    )
    expect(await run('agent', ['auth.s3cret'], undefined)).toEqual(
      refusal(CloseCode.unsupportedVersion),
    )
  })

  test('refuses a path version this relay does not speak, v1 included', async () => {
    expect(
      await run('agent', agentProtocols(TOKEN), undefined, {
        pathname: `/v1/agent/${code}`,
      }),
    ).toEqual(refusal(CloseCode.unsupportedVersion))
  })

  test('refuses a code that is not a code', async () => {
    expect(
      await run('agent', agentProtocols(TOKEN), undefined, {
        pathname: '/v2/agent/lowercase',
      }),
    ).toEqual(refusal(CloseCode.notFound))
  })

  test('a refusal echoes the version token so the handshake completes', async () => {
    const result = await run('agent', agentProtocols('nope'), undefined)
    expect(result).toEqual({
      _tag: 'Refuse',
      close: CloseCode.unauthorized,
      echo: Option.some('cycling.v2'),
    })
  })

  test('a refusal of an unknown version echoes its first non-credential token', async () => {
    const result = await run(
      'agent',
      ['cycling.v9', `auth.${TOKEN}`],
      undefined,
    )
    expect(result).toEqual({
      _tag: 'Refuse',
      close: CloseCode.unsupportedVersion,
      echo: Option.some('cycling.v9'),
    })
  })

  test.each([
    ['missing', Option.none()],
    ['empty', Option.some('')],
    ['short', Option.some('short')],
  ] as const)(
    'with a %s AGENT_TOKEN no agent is admitted, not even one offering the same or an empty token',
    async (_label, agentToken) => {
      for (const offered of [
        agentProtocols(TOKEN),
        agentProtocols(agentToken.pipe(Option.getOrElse(() => ''))),
        ['cycling.v2', 'auth.'],
      ]) {
        expect(await run('agent', offered, undefined, { agentToken })).toEqual(
          refusal(CloseCode.unauthorized),
        )
      }
    },
  )

  test('a bad AGENT_TOKEN does not stop browsers', async () => {
    const result = await run(
      'browser',
      browserProtocols(key),
      'https://cycling.computer',
      { agentToken: Option.none() },
    )
    expect(result._tag).toBe('Admit')
  })
})
