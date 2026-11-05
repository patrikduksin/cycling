interface ImportMetaEnv {
  /** WebSocket (or http) origin of the relay. alchemy sets it at build; in dev, pass it on the vite command line. */
  readonly VITE_RELAY_URL?: string
}
