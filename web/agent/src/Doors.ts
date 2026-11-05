import * as Match from 'effect/Match'

/**
 * A real ttyACM is one device file. Here two local doors share one remote
 * stream, so exactly one is active: the RFC 2217 door while a client holds
 * it (esptool), otherwise the pty. `.local/usb.lock` already keeps devtools
 * to one tool at a time, so the inactive door only ever sees strays.
 */
export type Door = 'pty' | 'rfc2217'

export interface Doors {
  readonly active: Door
  /** Bytes written by a door while it was inactive. */
  readonly dropped: Readonly<Record<Door, number>>
}

export const initialDoors: Doors = {
  active: 'pty',
  dropped: { pty: 0, rfc2217: 0 },
}

export type DoorEvent =
  | { readonly _tag: 'FromDevice'; readonly bytes: Uint8Array }
  | {
      readonly _tag: 'FromDoor'
      readonly door: Door
      readonly bytes: Uint8Array
    }
  | { readonly _tag: 'Rfc2217Client'; readonly connected: boolean }

export type Delivery =
  | { readonly _tag: 'ToDoor'; readonly door: Door; readonly bytes: Uint8Array }
  | { readonly _tag: 'ToDevice'; readonly bytes: Uint8Array }

export const route = (
  doors: Doors,
  event: DoorEvent,
): readonly [Doors, Delivery | null] =>
  Match.valueTags(event, {
    FromDevice: ({ bytes }): readonly [Doors, Delivery] => [
      doors,
      { _tag: 'ToDoor', door: doors.active, bytes },
    ],
    FromDoor: ({ door, bytes }): readonly [Doors, Delivery | null] =>
      door === doors.active
        ? [doors, { _tag: 'ToDevice', bytes }]
        : [
            {
              ...doors,
              dropped: {
                ...doors.dropped,
                [door]: doors.dropped[door] + bytes.length,
              },
            },
            null,
          ],
    Rfc2217Client: ({ connected }): readonly [Doors, null] => [
      { ...doors, active: connected ? 'rfc2217' : 'pty' },
      null,
    ],
  })
