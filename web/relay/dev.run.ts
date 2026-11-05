import * as Alchemy from 'alchemy'
import * as Effect from 'effect/Effect'

import * as Cloudflare from '@alchemy.run/cloudflare'

import { Relay } from './src/index.ts'

export default Alchemy.Stack(
  'RelayLocal',
  { providers: Cloudflare.providers(), state: Alchemy.localState() },
  Effect.gen(function* () {
    const relay = yield* Relay
    return { url: relay.url.as<string>() }
  }),
)
