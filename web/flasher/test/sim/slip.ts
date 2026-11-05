const END = 0xc0
const ESC = 0xdb
const ESC_END = 0xdc
const ESC_ESC = 0xdd

export const slipEncode = (payload: Uint8Array): Uint8Array => {
  const out = new Uint8Array(payload.length * 2 + 2)
  let at = 0
  out[at++] = END
  for (const byte of payload) {
    if (byte === END) {
      out[at++] = ESC
      out[at++] = ESC_END
    } else if (byte === ESC) {
      out[at++] = ESC
      out[at++] = ESC_ESC
    } else {
      out[at++] = byte
    }
  }
  out[at++] = END
  return out.slice(0, at)
}

/** Bytes outside a frame are dropped, as the ROM drops them. */
export class SlipDecoder {
  private frame: Array<number> | undefined
  private escaping = false

  push(bytes: Uint8Array): ReadonlyArray<Uint8Array> {
    const frames: Array<Uint8Array> = []
    for (const byte of bytes) {
      if (byte === END) {
        if (this.frame !== undefined && this.frame.length > 0) {
          frames.push(Uint8Array.from(this.frame))
        }
        this.frame = []
        this.escaping = false
      } else if (this.frame === undefined) {
        continue
      } else if (this.escaping) {
        this.frame.push(byte === ESC_END ? END : byte === ESC_ESC ? ESC : byte)
        this.escaping = false
      } else if (byte === ESC) {
        this.escaping = true
      } else {
        this.frame.push(byte)
      }
    }
    return frames
  }

  reset(): void {
    this.frame = undefined
    this.escaping = false
  }
}
