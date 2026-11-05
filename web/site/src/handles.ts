import { ManagedResource } from 'foldkit'

import type { RelayLink, SerialLink } from './links.ts'

export const SerialHandle = ManagedResource.tag<SerialLink>()('SerialHandle')
export const RelayHandle = ManagedResource.tag<RelayLink>()('RelayHandle')

export type HandleServices =
  | ManagedResource.ServiceOf<typeof SerialHandle>
  | ManagedResource.ServiceOf<typeof RelayHandle>
