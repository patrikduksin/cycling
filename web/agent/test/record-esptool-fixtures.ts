import * as Effect from 'effect/Effect'
import * as Fiber from 'effect/Fiber'
import * as Ref from 'effect/Ref'
import * as Schedule from 'effect/Schedule'
import * as Schema from 'effect/Schema'
import * as Stream from 'effect/Stream'
/**
 * Records what real esptool and pyserial send to our RFC 2217 door, into
 * test/fixtures. Needs the devtools Python env; no device is touched (the
 * door sits on a fake RemotePort). Run from anywhere:
 *
 *   bun web/agent/test/record-esptool-fixtures.ts
 */
import { resolve } from 'node:path'

import { serve } from '../src/rfc2217/Rfc2217Door.ts'
import { Fixture, fixturePath, tokenOf } from './support/controls.ts'
import {
  BAUD_115200,
  C606_IDENTITY,
  EPOCH_1,
  fakePort,
  openStatus,
} from './support/fakePort.ts'

const ROOT = resolve(import.meta.dir, '../../..')
const URL = '{url}'
const devtoolsPython = [
  'uv',
  'run',
  '--project',
  'tools/devtools',
  '--locked',
  'python',
]

const OPEN_ONLY = [
  'import sys, serial',
  'port = serial.serial_for_url(sys.argv[1], do_not_open=True)',
  'port.open()',
  'port.close()',
].join('; ')

const HARD_RESET = [
  'import sys, serial, esptool.reset',
  'port = serial.serial_for_url(sys.argv[1], do_not_open=True)',
  'port.open()',
  'esptool.reset.HardReset(port)()',
  'port.close()',
].join('; ')

const SCENARIOS: ReadonlyArray<{
  readonly name: string
  readonly description: string
  readonly command: ReadonlyArray<string>
}> = [
  {
    name: 'esptool-usb-reset',
    description:
      'esptool chip-id with --before usb-reset against the door; sync fails on the fake device after the reset',
    command: [
      ...devtoolsPython,
      '-m',
      'esptool',
      '--chip',
      'esp32s3',
      '--port',
      URL,
      '--before',
      'usb-reset',
      '--after',
      'no-reset',
      '--connect-attempts',
      '1',
      'chip-id',
    ],
  },
  {
    name: 'pyserial-hard-reset',
    description:
      "esptool's HardReset (its --after hard-reset) after a plain pyserial open",
    command: [...devtoolsPython, '-c', HARD_RESET, URL],
  },
  {
    name: 'pyserial-open-only',
    description: 'a plain pyserial open and close, no reset',
    command: [...devtoolsPython, '-c', OPEN_ONLY, URL],
  },
]

const encodeFixture = Schema.encodeSync(Schema.fromJsonString(Fixture))

const program = Effect.gen(function* () {
  const fake = yield* fakePort(openStatus(EPOCH_1, BAUD_115200))
  const tokens = yield* Ref.make<ReadonlyArray<string>>([])
  const disconnects = yield* Ref.make(0)
  const door = yield* serve({
    port: fake.port,
    identity: C606_IDENTITY,
    onRequest: request =>
      Ref.update(tokens, all => {
        const token = tokenOf(request)
        return token === null ? all : [...all, token]
      }),
  })
  yield* Effect.forkScoped(
    door.events.pipe(
      Stream.runForEach(event =>
        event._tag === 'Disconnected'
          ? Ref.update(disconnects, n => n + 1)
          : Effect.void,
      ),
    ),
  )
  for (const scenario of SCENARIOS) {
    yield* Ref.set(tokens, [])
    const before = yield* Ref.get(disconnects)
    const argv = scenario.command.map(arg => (arg === URL ? door.url : arg))
    const child = Bun.spawn([...argv], {
      cwd: ROOT,
      stdout: 'pipe',
      stderr: 'pipe',
    })
    const [output, errors, code] = yield* Effect.promise(() =>
      Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited,
      ]),
    )
    const disconnected = yield* Effect.forkChild(
      Effect.repeat(Ref.get(disconnects), {
        until: n => n > before,
        schedule: Schedule.spaced('20 millis'),
      }),
    )
    yield* Fiber.join(disconnected).pipe(Effect.timeout('5 seconds'))
    const controls = (yield* Ref.get(tokens)).join(' ')
    const fixture: Fixture = {
      description: scenario.description,
      command: [...scenario.command],
      tools: 'esptool 5.4.0, pyserial 3.5 (tools/devtools uv.lock)',
      controls,
    }
    yield* Effect.promise(() =>
      Bun.write(fixturePath(scenario.name), `${encodeFixture(fixture)}\n`),
    )
    console.log(`== ${scenario.name} (exit ${code})`)
    console.log(`${output}${errors}`.trim())
    console.log(`controls: ${controls}`)
  }
  console.log(
    `resets the fake port received: ${(yield* fake.resets).join(', ')}`,
  )
})

Effect.runPromise(Effect.scoped(program)).then(
  () => process.exit(0),
  error => {
    console.error(error)
    process.exit(1)
  },
)
