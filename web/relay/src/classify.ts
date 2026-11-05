import * as Effect from 'effect/Effect'
import * as Match from 'effect/Match'

import {
  decodeFromAgent,
  decodeFromBrowser,
  type Frame,
  type ProtocolViolation,
  type Role,
  type Sends,
} from '@cycling/protocol'

import type { Event } from './decide.ts'

/**
 * How one role's text frames become events: the decoder for what that role may
 * send, and the event that carries the decoded message with its raw text.
 */
const sender = <M>(
  from: Role,
  decode: (text: string) => Effect.Effect<Frame<M>, ProtocolViolation>,
  textEvent: (message: M, text: string) => Event,
) => {
  const malformed: Event = { _tag: 'Malformed', from }
  return (text: string): Effect.Effect<Event> =>
    decode(text).pipe(
      Effect.match({
        onSuccess: frame =>
          Match.valueTags(frame, {
            Control: ({ message }): Event => textEvent(message, text),
            Pong: (): Event => ({ _tag: 'Pong' }),
            Console: (): Event => malformed,
            OpData: (): Event => malformed,
          }),
        onFailure: () => malformed,
      }),
    )
}

const SENDERS: Readonly<Record<Role, (text: string) => Effect.Effect<Event>>> =
  {
    browser: sender<Sends<'browser'>>(
      'browser',
      decodeFromBrowser,
      (message, text) => ({ _tag: 'BrowserText', message, text }),
    ),
    agent: sender<Sends<'agent'>>(
      'agent',
      decodeFromAgent,
      (message, text) => ({
        _tag: 'AgentText',
        message,
        text,
      }),
    ),
  }

/**
 * Turns one inbound text frame into an event. It is decoded with the sender's
 * own union, so a message from the wrong direction, or one with an extra
 * property such as a `serialNumber`, is malformed. Binary frames never come
 * here: they are device bytes, and the Durable Object only measures them.
 */
export const classify = (from: Role, text: string): Effect.Effect<Event> =>
  SENDERS[from](text)
