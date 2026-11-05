import { describe, expect, test } from 'bun:test'

import { greet, receive, type Session } from '../src/rfc2217/ComPort.ts'
import {
  escapeData,
  initialParser,
  type Item,
  parse,
  subnegotiation,
} from '../src/rfc2217/Telnet.ts'

const IAC = 0xff
const SB = 0xfa
const SE = 0xf0
const WILL = 0xfb
const WONT = 0xfc
const DO = 0xfd
const COM = 44

const parseAll = (chunks: ReadonlyArray<ReadonlyArray<number>>) => {
  let parser = initialParser
  const items: Array<Item> = []
  for (const chunk of chunks) {
    const result = parse(parser, Uint8Array.from(chunk))
    parser = result.parser
    items.push(...result.items)
  }
  return items
}

const dataOf = (items: ReadonlyArray<Item>) =>
  items.flatMap(item => (item._tag === 'Data' ? [...item.bytes] : []))

describe('telnet', () => {
  test('a doubled IAC from the client is one 0xff data byte, even split across reads', () => {
    expect(dataOf(parseAll([[0x41, IAC, IAC, 0x42]]))).toEqual([
      0x41, 0xff, 0x42,
    ])
    expect(
      dataOf(
        parseAll([
          [0x41, IAC],
          [IAC, 0x42],
        ]),
      ),
    ).toEqual([0x41, 0xff, 0x42])
  })

  test('device bytes to the client double every IAC and nothing else', () => {
    const all = Uint8Array.from({ length: 256 }, (_, i) => i)
    const escaped = escapeData(all)
    expect(escaped.length).toBe(257)
    expect(dataOf(parseAll([[...escaped]]))).toEqual([...all])
  })

  test('negotiation and subnegotiation are separated from data, IAC inside SB undoubled', () => {
    const items = parseAll([
      [0x01, IAC, DO, COM, 0x02, IAC, SB, COM, 1, 0, IAC],
      [IAC, 0, 0, IAC, SE, 0x03],
    ])
    expect(items).toEqual([
      { _tag: 'Data', bytes: Uint8Array.of(1) },
      { _tag: 'Negotiate', verb: 'DO', option: COM },
      { _tag: 'Data', bytes: Uint8Array.of(2) },
      { _tag: 'Subnegotiation', payload: Uint8Array.of(COM, 1, 0, IAC, 0, 0) },
      { _tag: 'Data', bytes: Uint8Array.of(3) },
    ])
  })

  test('a subnegotiation value with 0xff is doubled on the wire', () => {
    expect([...subnegotiation(Uint8Array.of(COM, 101, 0xff))]).toEqual([
      IAC,
      SB,
      COM,
      101,
      IAC,
      IAC,
      IAC,
      SE,
    ])
  })
})

const run = (
  session: Session,
  wire: ReadonlyArray<number>,
): { session: Session; reply: Array<number>; requests: Array<unknown> } => {
  let current = session
  const reply: Array<number> = []
  const requests: Array<unknown> = []
  for (const item of parse(initialParser, Uint8Array.from(wire)).items) {
    const step = receive(current, item)
    current = step.session
    for (const chunk of step.reply) reply.push(...chunk)
    if (step.request !== null) requests.push(step.request)
  }
  return { session: current, reply, requests }
}

const sub = (...payload: Array<number>) => [IAC, SB, COM, ...payload, IAC, SE]

describe('com port', () => {
  const { session } = greet(115200)

  test('pyserial opening requests are answered so its negotiation completes', () => {
    const SGA = 3
    const ECHO = 1
    const result = run(session, [
      IAC,
      DO,
      ECHO,
      IAC,
      WILL,
      SGA,
      IAC,
      DO,
      SGA,
      IAC,
      DO,
      COM,
      IAC,
      WILL,
      COM,
    ])
    expect(result.reply).toEqual([IAC, WONT, ECHO, IAC, DO, SGA])
  })

  test('port settings are acknowledged with the server code and the same value', () => {
    expect(run(session, sub(1, 0, 1, 0xc2, 0)).reply).toEqual(
      sub(101, 0, 1, 0xc2, 0),
    )
    expect(run(session, sub(2, 8)).reply).toEqual(sub(102, 8))
    expect(run(session, sub(3, 1)).reply).toEqual(sub(103, 1))
    expect(run(session, sub(4, 1)).reply).toEqual(sub(104, 1))
    expect(run(session, sub(5, 1)).reply).toEqual(sub(105, 1))
    expect(run(session, sub(12, 1)).reply).toEqual(sub(112, 1))
  })

  test('a baud change surfaces once; a zero baud is a query for the current one', () => {
    expect(run(session, sub(1, 0, 0x0e, 0x10, 0)).requests).toEqual([
      { _tag: 'SetBaud', baud: 921600 },
    ])
    const query = run(session, sub(1, 0, 0, 0, 0))
    expect(query.requests).toEqual([])
    expect(query.reply).toEqual(sub(101, 0, 1, 0xc2, 0))
  })

  test('DTR and RTS commands surface as line commands and are acked', () => {
    const result = run(session, [...sub(5, 8), ...sub(5, 12), ...sub(12, 3)])
    expect(result.requests).toEqual([
      { _tag: 'Line', command: { line: 'dtr', on: true } },
      { _tag: 'Line', command: { line: 'rts', on: false } },
      { _tag: 'Purge' },
    ])
    expect(result.reply).toEqual([
      ...sub(105, 8),
      ...sub(105, 12),
      ...sub(112, 3),
    ])
    expect(run(result.session, sub(5, 7)).reply).toEqual(sub(105, 8))
  })

  test('client data passes through untouched', () => {
    expect(run(session, [0x00, 0x0d, 0x0a, IAC, IAC]).requests).toEqual([
      { _tag: 'Data', bytes: Uint8Array.of(0x00, 0x0d, 0x0a, 0xff) },
    ])
  })
})
