import * as Clock from 'effect/Clock'
import * as Effect from 'effect/Effect'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'
import { HttpServerRequest } from 'effect/http/HttpServerRequest'
import * as HttpServerResponse from 'effect/http/HttpServerResponse'

import * as Cloudflare from '@alchemy.run/cloudflare'
import {
  CloseCode,
  encodeRelayMsg,
  KEEPALIVE,
  MAX_FRAME_BYTES,
  Role,
  SUBPROTOCOL,
} from '@cycling/protocol'

import { SessionRecord } from './SessionRecord.ts'
import { classify } from './classify.ts'
import { closeReason } from './close.ts'
import {
  type Action,
  type Audience,
  decide,
  type Decision,
  type Event,
  otherRole,
} from './decide.ts'
import { OWNER_HEADER, ROLE_HEADER } from './headers.ts'
import { encodeTag, parseTag } from './tags.ts'

const RECORD_KEY = 'record'

const decodeRole = Schema.decodeUnknownOption(Role)
const decodeRecord = Schema.decodeUnknownEffect(SessionRecord)
const encodeRecord = Schema.encodeEffect(SessionRecord)

interface Subject {
  readonly socket: Cloudflare.WebSocket
  readonly id: string
  readonly role: Role
}

/** Cloudflare throws a TypeError from send() on a socket that is already closing or closed. */
const sentAfterClose = (defect: unknown): boolean =>
  defect instanceof TypeError &&
  defect.message.includes("Can't call WebSocket send() after close()")

const quietly = <A>(effect: Effect.Effect<A>): Effect.Effect<void> =>
  effect.pipe(
    Effect.asVoid,
    Effect.catchDefect(defect =>
      sentAfterClose(defect) ? Effect.void : Effect.die(defect),
    ),
  )

/** 1005, 1006 and 1015 are reported to handlers but cannot be sent in a close frame. */
const echoable = (code: number): number =>
  code === 1005 || code === 1006 || code === 1015 ? 1000 : code

/**
 * One Durable Object per session code. Its durable state is one record, the
 * alarm, and per-socket tags; nothing lives in memory, so a hibernation wake
 * reruns the inner gen and every handler reads what it needs. The policy is
 * `decide`; this class gathers the facts for it and carries out the result.
 */
export default class Session extends Cloudflare.DurableObject<Session>()(
  'Sessions',
  Effect.map(Effect.service(Cloudflare.DurableObjectState), state =>
    Effect.gen(function* () {
      yield* state.setWebSocketAutoResponse(
        new WebSocketRequestResponsePair(KEEPALIVE.ping, KEEPALIVE.pong),
      )

      const readRecord = Effect.gen(function* () {
        const stored = yield* state.storage.get(RECORD_KEY)
        return stored === undefined
          ? null
          : yield* decodeRecord(stored).pipe(Effect.orDie)
      })

      const writeRecord = (record: SessionRecord | null) =>
        record === null
          ? state.storage.delete(RECORD_KEY)
          : encodeRecord(record).pipe(
              Effect.orDie,
              Effect.flatMap(encoded => state.storage.put(RECORD_KEY, encoded)),
            )

      const liveSockets = (role: Role, exceptId: string | null) =>
        state
          .getWebSockets(role)
          .pipe(
            Effect.flatMap(sockets =>
              Effect.filter(sockets, socket =>
                state
                  .getTags(socket.ws)
                  .pipe(
                    Effect.map(
                      tags =>
                        exceptId === null ||
                        !Option.exists(
                          parseTag(tags),
                          tag => tag._tag === 'Live' && tag.id === exceptId,
                        ),
                    ),
                  ),
              ),
            ),
          )

      const presenceOf = (exceptId: string | null) =>
        Effect.all({
          browser: liveSockets('browser', exceptId),
          agent: liveSockets('agent', exceptId),
        }).pipe(
          Effect.map(({ browser, agent }) => ({
            browser: browser.length > 0,
            agent: agent.length > 0,
          })),
        )

      const subjectOf = (socket: Cloudflare.WebSocket) =>
        state
          .getTags(socket.ws)
          .pipe(
            Effect.map((tags): Option.Option<Subject> =>
              parseTag(tags).pipe(
                Option.flatMap(tag =>
                  tag._tag === 'Live'
                    ? Option.some({ socket, id: tag.id, role: tag.role })
                    : Option.none(),
                ),
              ),
            ),
          )

      const audience = (to: Audience, subject: Subject | null) =>
        Match.valueTags(to, {
          Subject: () =>
            Effect.succeed(subject === null ? [] : [subject.socket]),
          Others: ({ role }) => liveSockets(role, subject?.id ?? null),
          Everyone: () =>
            Effect.all([
              liveSockets('browser', null),
              liveSockets('agent', null),
            ]).pipe(Effect.map(groups => groups.flat())),
        })

      const deliver = <R>(
        sockets: Effect.Effect<ReadonlyArray<Cloudflare.WebSocket>, never, R>,
        send: (socket: Cloudflare.WebSocket) => Effect.Effect<void>,
      ) =>
        sockets.pipe(
          Effect.flatMap(found =>
            Effect.forEach(found, socket => quietly(send(socket))),
          ),
        )

      const closeSocket = (socket: Cloudflare.WebSocket, code: number) =>
        socket.close(code, closeReason(code))

      const perform = (action: Action, subject: Subject | null) =>
        Match.valueTags(action, {
          Send: ({ to, message }) => {
            const text = encodeRelayMsg(message)
            return deliver(audience(to, subject), socket => socket.send(text))
          },
          Forward: ({ to, text }) =>
            deliver(liveSockets(to, subject?.id ?? null), socket =>
              socket.send(text),
            ),
          Close: ({ who, code }) =>
            deliver(audience(who, subject), socket =>
              closeSocket(socket, code),
            ),
          SetAlarm: ({ at }) => state.storage.setAlarm(at),
          ClearAlarm: () => state.storage.deleteAlarm(),
        })

      const apply = (
        before: SessionRecord | null,
        decision: Decision,
        subject: Subject | null,
      ) =>
        Effect.gen(function* () {
          if (decision.record !== before) {
            yield* writeRecord(decision.record)
          }
          yield* Effect.forEach(decision.actions, action =>
            perform(action, subject),
          )
        })

      const handle = (event: Event, subject: Subject | null) =>
        Effect.gen(function* () {
          const record = yield* readRecord
          const presence = yield* presenceOf(subject?.id ?? null)
          const now = yield* Clock.currentTimeMillis
          const decision = decide({ record, presence }, event, now)
          yield* apply(record, decision, subject)
        })

      /** Device bytes touch no state: pass them on untouched, or close a sender that breaks the frame limit. */
      const relayBinary = (subject: Subject, bytes: ArrayBuffer) =>
        bytes.byteLength > MAX_FRAME_BYTES
          ? quietly(closeSocket(subject.socket, CloseCode.protocolViolation))
          : deliver(liveSockets(otherRole(subject.role), subject.id), socket =>
              socket.send(new Uint8Array(bytes)),
            )

      const upgrade = Effect.gen(function* () {
        const request = yield* HttpServerRequest
        const role = decodeRole(request.headers[ROLE_HEADER])
        const ownerKeyHash = request.headers[OWNER_HEADER]
        if (Option.isNone(role)) {
          return HttpServerResponse.text('missing role', { status: 400 })
        }
        const event: Event | null =
          role.value === 'agent'
            ? { _tag: 'AgentUpgrade' }
            : ownerKeyHash === undefined
              ? null
              : { _tag: 'BrowserUpgrade', ownerKeyHash }
        if (event === null) {
          return HttpServerResponse.text('missing owner', { status: 400 })
        }

        const record = yield* readRecord
        const presence = yield* presenceOf(null)
        const now = yield* Clock.currentTimeMillis
        const decision = decide({ record, presence }, event, now)
        const refused = decision.actions.some(
          action => action._tag === 'Close' && action.who._tag === 'Subject',
        )
        const id = crypto.randomUUID()
        const [response, socket] = yield* Cloudflare.upgrade({
          tags: encodeTag(
            refused
              ? { _tag: 'Refused' }
              : { _tag: 'Live', role: role.value, id },
          ),
        })
        yield* apply(record, decision, { socket, id, role: role.value })
        return HttpServerResponse.setHeader(
          response,
          'sec-websocket-protocol',
          SUBPROTOCOL,
        )
      })

      const closed = (socket: Cloudflare.WebSocket, code: number) =>
        Effect.gen(function* () {
          const subject = yield* subjectOf(socket)
          if (Option.isSome(subject)) {
            yield* handle(
              { _tag: 'Closed', role: subject.value.role },
              subject.value,
            )
          }
          yield* quietly(closeSocket(socket, echoable(code)))
        })

      return {
        fetch: upgrade,

        webSocketMessage: Effect.fn(function* (
          socket: Cloudflare.WebSocket,
          message: string | ArrayBuffer,
        ) {
          const subject = yield* subjectOf(socket)
          if (Option.isNone(subject)) {
            return
          }
          yield* typeof message === 'string'
            ? classify(subject.value.role, message).pipe(
                Effect.flatMap(event => handle(event, subject.value)),
              )
            : relayBinary(subject.value, message)
        }),

        webSocketClose: Effect.fn(function* (
          socket: Cloudflare.WebSocket,
          code: number,
          _reason: string,
        ) {
          yield* closed(socket, code)
        }),

        webSocketError: Effect.fn(function* (
          socket: Cloudflare.WebSocket,
          _error: unknown,
        ) {
          yield* closed(socket, 1011)
        }),

        alarm: () => handle({ _tag: 'Alarm' }, null),
      }
    }),
  ),
) {}
