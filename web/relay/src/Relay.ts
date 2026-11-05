import { Stage } from 'alchemy/Stage'
import * as Config from 'effect/Config'
import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Redacted from 'effect/Redacted'
import * as Headers from 'effect/http/Headers'
import * as HttpBody from 'effect/http/HttpBody'
import { HttpServerRequest } from 'effect/http/HttpServerRequest'
import * as HttpServerResponse from 'effect/http/HttpServerResponse'

import * as Cloudflare from '@alchemy.run/cloudflare'
import { CloseCode, parseSessionPath, SUBPROTOCOL } from '@cycling/protocol'

import Session from './Session.ts'
import {
  admit,
  AGENT_TOKEN_MIN_LENGTH,
  parseAgentToken,
  parseOrigins,
} from './admission.ts'
import { closeReason } from './close.ts'
import { OWNER_HEADER, ROLE_HEADER } from './headers.ts'

const PRODUCTION_ORIGIN = 'https://cycling.computer'
const RELAY_HOST = 'relay.cycling.computer'

/**
 * Browser socket opens per client address. Create and resume look the same
 * here, because telling them apart would mean waking the Session, so the
 * ceiling leaves room for a few page reloads and a reconnect backoff.
 */
const SESSION_RATE = { limit: 20, period: 60 } as const

const refused = (
  close: number,
  protocol: Option.Option<string>,
): Effect.Effect<HttpServerResponse.HttpServerResponse> =>
  Effect.sync(() => {
    const pair = new WebSocketPair()
    const client = pair[0]
    const server = pair[1]
    server.accept()
    server.close(close, closeReason(close))
    const init = {
      status: 101,
      webSocket: client,
      headers: Option.match(protocol, {
        onNone: () => ({}),
        onSome: value => ({ 'sec-websocket-protocol': value }),
      }),
    }
    return HttpServerResponse.setBody(
      HttpServerResponse.empty({ status: 101 }),
      HttpBody.raw(new Response(null, init)),
    )
  })

/**
 * The public edge of the relay.
 *
 *   GET /v1/browser/:code   subprotocols: cycling.v1, own.<ownerKey>
 *   GET /v1/agent/:code     subprotocols: cycling.v1, auth.<token>
 *
 * Everything that can be decided without the Session is decided here, so a
 * stranger's request never wakes a Durable Object: version, code shape,
 * credentials, origin, rate limit. The Session then trusts the role header
 * set here and strips every credential from what it sees.
 */
export default class Relay extends Cloudflare.Worker<Relay>()(
  'Relay',
  Stage.pipe(
    Effect.map(stage => ({
      main: import.meta.url,
      /**
       * Credentials ride in `Sec-WebSocket-Protocol`, which invocation logs
       * record, so nothing about a request is logged or kept.
       */
      observability: {
        enabled: false,
        logs: { enabled: false, invocationLogs: false },
        traces: { enabled: false },
      },
      ...(stage === 'prod' ? { domain: RELAY_HOST } : {}),
    })),
  ),
  Effect.gen(function* () {
    const sessions = yield* Session
    const agentToken = parseAgentToken(
      (yield* Config.option(Config.Redacted('AGENT_TOKEN'))).pipe(
        Option.map(Redacted.value),
      ),
    )
    if (Option.isNone(agentToken)) {
      yield* Effect.logError(
        'AGENT_TOKEN is missing or too short; every agent upgrade is refused',
      ).pipe(
        Effect.annotateLogs({
          binding: 'AGENT_TOKEN',
          minLength: AGENT_TOKEN_MIN_LENGTH,
        }),
      )
    }
    const allowed = yield* Config.String('ALLOWED_ORIGINS').pipe(
      Config.withDefault(PRODUCTION_ORIGIN),
    )
    const stage = yield* Config.String('ALCHEMY_STAGE').pipe(
      Config.withDefault('prod'),
    )
    const sessionRate = yield* Cloudflare.RateLimit('SESSION_RATE', {
      namespaceId: 1001,
      simple: SESSION_RATE,
    })
    const credentials = {
      agentToken,
      origins: { allowed: parseOrigins(allowed), allowLocal: stage !== 'prod' },
    }

    return {
      fetch: Effect.gen(function* () {
        const request = yield* HttpServerRequest
        const route = parseSessionPath(request.url.split('?')[0] ?? '')
        if (Option.isNone(route)) {
          return HttpServerResponse.text('not found', { status: 404 })
        }
        if (request.headers.upgrade?.toLowerCase() !== 'websocket') {
          return HttpServerResponse.text('expected a WebSocket upgrade', {
            status: 426,
            headers: { upgrade: 'websocket' },
          })
        }

        const admission = yield* admit(
          {
            route: route.value,
            protocolHeader: request.headers['sec-websocket-protocol'] ?? null,
            origin: request.headers.origin,
          },
          credentials,
        )
        if (admission._tag === 'Refuse') {
          return yield* refused(admission.close, admission.echo)
        }

        if (admission.role === 'browser') {
          const key = request.headers['cf-connecting-ip'] ?? 'unknown'
          const verdict = yield* sessionRate.limit({ key }).pipe(
            Effect.map(({ success }) => success),
            Effect.orElseSucceed(() => true),
          )
          if (!verdict) {
            return yield* refused(
              CloseCode.rateLimited,
              Option.some(SUBPROTOCOL),
            )
          }
        }

        const stripped = Headers.remove(
          Headers.remove(request.headers, 'sec-websocket-protocol'),
          OWNER_HEADER,
        )
        const withRole = Headers.set(stripped, ROLE_HEADER, admission.role)
        const forwarded =
          admission.role === 'browser'
            ? Headers.set(withRole, OWNER_HEADER, admission.ownerKeyHash)
            : withRole
        return yield* sessions
          .getByName(admission.code)
          .fetch(request.modify({ headers: forwarded }))
      }),
    }
  }).pipe(Effect.provide(Cloudflare.Workers.RateLimitBinding)),
) {}
