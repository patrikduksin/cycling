import { SimDevice } from './device.ts'
import { SLOT_A, stockImage, stockLayout } from './image.ts'
import { FakeSerial } from './port.ts'

/**
 * Load before the page: a simulated C606 on a stock layout becomes
 * `navigator.serial`, and `globalThis.__sim` drives chaos runs. Slot A's
 * image lands a moment later (WebCrypto's SHA-256 is async), long before a
 * visitor can pick the port. `?sim-read-kbps=140` paces READ_FLASH like the
 * C606.
 */
const kbps = Number(new URLSearchParams(location.search).get('sim-read-kbps'))
const device = new SimDevice({
  flash: stockLayout(),
  readBytesPerSecond: kbps > 0 ? kbps * 1000 : Infinity,
})
void stockImage().then(image => device.flash.set(image, SLOT_A))

Object.defineProperty(navigator, 'serial', {
  configurable: true,
  value: new FakeSerial(device),
})

Object.defineProperty(globalThis, '__sim', {
  configurable: true,
  value: {
    unplug: () => device.unplug(),
    flashMd5: (address: number, size: number) => device.flashMd5(address, size),
    state: () => device.state(),
  },
})
