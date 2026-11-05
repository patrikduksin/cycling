import * as Option from 'effect/Option'

import { closeName } from '@cycling/protocol'

export const closeReason = (code: number): string =>
  Option.getOrElse(closeName(code), () => '')
