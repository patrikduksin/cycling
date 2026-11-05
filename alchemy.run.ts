import * as Alchemy from 'alchemy'
import * as Output from 'alchemy/Output'
import { Stage } from 'alchemy/Stage'
import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import { resolve } from 'node:path'

import * as Cloudflare from '@alchemy.run/cloudflare'

import { Relay } from './web/relay/src/index.ts'

/**
 * Only the `prod` stage owns the real hostnames and the shared remote state
 * store. Every other stage (dev, test) runs with no custom domain and state
 * on local disk, so it can never touch what prod serves.
 */
const state = Layer.unwrap(
  Stage.pipe(
    Effect.map(stage =>
      stage === 'prod' ? Cloudflare.state() : Alchemy.localState(),
    ),
  ),
)

export default Alchemy.Stack(
  'Cycling',
  { providers: Cloudflare.providers(), state },
  Effect.gen(function* () {
    const stage = yield* Stage
    const relay = yield* Relay
    const relayUrl = relay.url
      .as<string>()
      .pipe(Output.map(url => url.replace(/^http/, 'ws')))
    const site = yield* Cloudflare.Website.Foldkit('Site', {
      rootDir: resolve(import.meta.dirname, 'web/site'),
      ...(stage === 'prod' ? { domain: 'cycling.computer' } : {}),
      env: { VITE_RELAY_URL: relayUrl },
    })
    return { relay: relayUrl, site: site.url.as<string>() }
  }),
)
