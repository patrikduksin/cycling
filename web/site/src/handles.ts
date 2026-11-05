import { ManagedResource } from 'foldkit'

import type { PortLink, RelayLink } from './links.ts'

export const PortHandle = ManagedResource.tag<PortLink>()('PortHandle')
export const RelayHandle = ManagedResource.tag<RelayLink>()('RelayHandle')

export type HandleServices =
  | ManagedResource.ServiceOf<typeof PortHandle>
  | ManagedResource.ServiceOf<typeof RelayHandle>
