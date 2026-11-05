import { describe, expect, test } from 'bun:test'
import * as Option from 'effect/Option'

import { encodeTag, parseTag, type SocketTag } from './tags.ts'

const live: SocketTag = { _tag: 'Live', role: 'agent', id: 'a1b2-c3' }

describe('socket tags', () => {
  test.each([
    live,
    { _tag: 'Live', role: 'browser', id: 'x' },
    { _tag: 'Refused' },
  ] as const)('%o survives a round trip', tag => {
    expect(parseTag(encodeTag(tag))).toEqual(Option.some(tag))
  })

  test('a live socket carries its bare role, so getWebSockets(role) finds it', () => {
    expect(encodeTag(live)).toContain('agent')
  })

  test('reads the tags in any order', () => {
    expect(parseTag([...encodeTag(live)].reverse())).toEqual(Option.some(live))
  })

  test.each([
    [],
    ['agent'],
    ['id:x'],
    ['robot', 'id:x'],
    ['agent', 'id:x', 'id:y'],
    ['agent', 'browser', 'id:x'],
    ['refused', 'id:x'],
    ['refused', 'agent'],
  ])('rejects %o', (...tags) => {
    expect(Option.isNone(parseTag(tags))).toBe(true)
  })
})
