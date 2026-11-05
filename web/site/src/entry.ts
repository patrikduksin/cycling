import * as Effect from 'effect/Effect'
import * as Layer from 'effect/Layer'
import * as Option from 'effect/Option'
import * as Socket from 'effect/socket/Socket'
import { Runtime } from 'foldkit'

import { browserRelay } from './adapters/relay.ts'
import { makeWebSerial } from './adapters/serial.ts'
import { webUsb } from './adapters/usb.ts'
import { Chooser } from './links.ts'
import type { Adapters } from './links.ts'
import { Message } from './message.ts'
import { type Environment, Flags, Model } from './model.ts'
import { parseRelayBase } from './policy.ts'
import { managedResources, subscriptions } from './resources.ts'
import { readStoredSession } from './storage.ts'
import { init, update } from './update.ts'
import { view } from './view.ts'

const relayBase = parseRelayBase(import.meta.env.VITE_RELAY_URL)

/** Dev-only switches: `?fake` swaps in a fake device and relay, `?unsupported` shows the Safari screen. */
const devQuery = import.meta.env.DEV
  ? new URLSearchParams(location.search)
  : undefined

const realAdapters: Adapters = {
  serial: makeWebSerial(),
  usb: webUsb,
  relay: Option.map(relayBase, base =>
    browserRelay({
      base,
      webSocket: Socket.layerWebSocketConstructorGlobal,
    }),
  ),
}

const loadAdapters = async (): Promise<Adapters> => {
  if (devQuery?.has('fake')) {
    const { fakeAdapters } = await import('./dev/fake.ts')
    return fakeAdapters
  }
  return realAdapters
}

const readEnvironment = (adapters: Adapters): Environment => {
  const fake = devQuery?.has('fake') === true
  return {
    hasWebSerial:
      devQuery?.has('unsupported') === true
        ? false
        : fake || 'serial' in navigator,
    hasWebUsb: fake || 'usb' in navigator,
    isSecureContext: window.isSecureContext,
    relayConfigured: Option.isSome(adapters.relay),
  }
}

const adapters = await loadAdapters()

const flags = readStoredSession.pipe(
  Effect.map(stored => ({ environment: readEnvironment(adapters), stored })),
)

Runtime.run(
  Runtime.makeApplication({
    Model,
    Flags,
    init,
    update,
    view,
    subscriptions,
    managedResources: managedResources(adapters),
    resources: Layer.succeed(Chooser, {
      serial: adapters.serial.choose,
      usb: adapters.usb.request,
    }),
    container: document.getElementById('root'),
    devTools: import.meta.env.DEV ? { Message } : false,
  }),
  { flags },
)
