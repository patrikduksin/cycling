import { describe, expect, test } from 'bun:test'

import {
  type Delivery,
  type DoorEvent,
  type Doors,
  initialDoors,
  route,
} from '../src/Doors.ts'

const bytes = Uint8Array.of(1, 2, 3)
const esptool: Doors = { ...initialDoors, active: 'rfc2217' }

const cases: ReadonlyArray<{
  readonly name: string
  readonly doors: Doors
  readonly event: DoorEvent
  readonly next: Doors
  readonly delivery: Delivery | null
}> = [
  {
    name: 'device bytes go to the pty while no rfc2217 client is connected',
    doors: initialDoors,
    event: { _tag: 'FromDevice', bytes },
    next: initialDoors,
    delivery: { _tag: 'ToDoor', door: 'pty', bytes },
  },
  {
    name: 'device bytes go to the rfc2217 client while it holds the door',
    doors: esptool,
    event: { _tag: 'FromDevice', bytes },
    next: esptool,
    delivery: { _tag: 'ToDoor', door: 'rfc2217', bytes },
  },
  {
    name: 'the active door writes to the device',
    doors: esptool,
    event: { _tag: 'FromDoor', door: 'rfc2217', bytes },
    next: esptool,
    delivery: { _tag: 'ToDevice', bytes },
  },
  {
    name: 'a stray pty write while esptool holds the door is dropped and counted',
    doors: esptool,
    event: { _tag: 'FromDoor', door: 'pty', bytes },
    next: { ...esptool, dropped: { pty: 3, rfc2217: 0 } },
    delivery: null,
  },
  {
    name: 'an rfc2217 client connecting takes the door',
    doors: initialDoors,
    event: { _tag: 'Rfc2217Client', connected: true },
    next: esptool,
    delivery: null,
  },
  {
    name: 'an rfc2217 client leaving gives the door back to the pty',
    doors: esptool,
    event: { _tag: 'Rfc2217Client', connected: false },
    next: initialDoors,
    delivery: null,
  },
]

describe('route', () => {
  for (const { name, doors, event, next, delivery } of cases) {
    test(name, () => {
      expect(route(doors, event)).toEqual([next, delivery])
    })
  }
})
