import { describe, expect, test } from 'bun:test'

import { FAMILIES } from '@cycling/flasher'
import { ChipFamily } from '@cycling/protocol'

describe('chip families', () => {
  test('the wire carries exactly the families the flasher has tables for', () => {
    expect<ReadonlyArray<string>>([...ChipFamily.literals].sort()).toEqual(
      Object.keys(FAMILIES).sort(),
    )
  })
})
