import { describe, expect, test } from 'bun:test'
import * as Schema from 'effect/Schema'

import type { ResetKind } from '@cycling/protocol'

import { initialRecognizer, step } from '../src/ResetRecognizer.ts'
import { Fixture, fixturePath, parseControls } from './support/controls.ts'

const recognize = (controls: string): ReadonlyArray<ResetKind> => {
  let state = initialRecognizer
  const fired: Array<ResetKind> = []
  for (const control of parseControls(controls)) {
    if ('purge' in control) continue
    const [next, kind] = step(state, control)
    state = next
    if (kind !== null) fired.push(kind)
  }
  return fired
}

const decodeFixture = Schema.decodeUnknownSync(Schema.fromJsonString(Fixture))
const recorded = async (name: string) =>
  decodeFixture(await Bun.file(fixturePath(name)).text()).controls

describe('reset recognizer on recorded esptool 5.4.0 sequences (R6)', () => {
  test('--before usb-reset is exactly one usbJtagBootloader', async () => {
    expect(recognize(await recorded('esptool-usb-reset'))).toEqual([
      'usbJtagBootloader',
    ])
  })

  test("esptool's HardReset after a plain open is exactly one hardReset", async () => {
    expect(recognize(await recorded('pyserial-hard-reset'))).toEqual([
      'hardReset',
    ])
  })

  test('an open with no reset fires nothing', async () => {
    expect(recognize(await recorded('pyserial-open-only'))).toEqual([])
  })
})

const USB_RESET_STEPS = 'R0 D1 D0 D1 R0 D1 R1 D1 D0 R1 D0 D0 R0 D0'

describe('reset recognizer rules', () => {
  test('an open with both lines low, as monitor() does, fires nothing', () => {
    expect(recognize('D0 R0 P P')).toEqual([])
  })

  test("esptool's next connect attempt is recognized again", () => {
    expect(
      recognize(`D1 R1 P P P ${USB_RESET_STEPS} ${USB_RESET_STEPS}`),
    ).toEqual(['usbJtagBootloader', 'usbJtagBootloader'])
  })

  test('a bootloader reset then --after hard-reset fires both, in order', () => {
    expect(recognize(`D1 R1 P P ${USB_RESET_STEPS} R1 D0 R0 D0`)).toEqual([
      'usbJtagBootloader',
      'hardReset',
    ])
  })

  test('an RTS pulse with a DTR change inside it is not a hard reset', () => {
    expect(recognize('D1 R1 P P R1 D0 R0')).toEqual([])
  })

  test('the command that first sets a line never arms a pulse', () => {
    expect(recognize('D1 R1 R0')).toEqual([])
  })
})
