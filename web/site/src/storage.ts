import * as Effect from 'effect/Effect'
import * as Option from 'effect/Option'
import * as Schema from 'effect/Schema'

import { StoredSession } from './model.ts'

const KEY = 'cycling.remote.session'

const StoredJson = Schema.fromJsonString(StoredSession)
const decode = Schema.decodeUnknownOption(StoredJson)
const encode = Schema.encodeSync(StoredJson)

/** Private browsing can make storage throw; a missing record just means a fresh start. */
export const readStoredSession: Effect.Effect<Option.Option<StoredSession>> =
  Effect.try(() => sessionStorage.getItem(KEY)).pipe(
    Effect.map(raw => Option.flatMap(Option.fromNullishOr(raw), decode)),
    Effect.orElseSucceed(() => Option.none()),
  )

export const writeStoredSession = (
  stored: StoredSession,
): Effect.Effect<void> =>
  Effect.try(() => sessionStorage.setItem(KEY, encode(stored))).pipe(
    Effect.ignore,
  )

export const clearStoredSession: Effect.Effect<void> = Effect.try(() =>
  sessionStorage.removeItem(KEY),
).pipe(Effect.ignore)
