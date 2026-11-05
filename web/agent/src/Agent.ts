import * as Arr from 'effect/Array'
import * as Console from 'effect/Console'
import * as Effect from 'effect/Effect'
import type * as FileSystem from 'effect/FileSystem'
import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import * as Redacted from 'effect/Redacted'
import * as Ref from 'effect/Ref'
import * as SubscriptionRef from 'effect/SubscriptionRef'
import * as ChildProcess from 'effect/process/ChildProcess'
import * as ChildProcessSpawner from 'effect/process/ChildProcessSpawner'

import { type Code, type Device, parseCode } from '@cycling/protocol'

import { AgentConfig } from './AgentConfig.ts'
import * as Bridge from './Bridge.ts'
import type { PtyHost } from './PtyHost.ts'
import { Remote } from './Remote.ts'
import { firstMatch, isPortOpen, over, type RemotePort } from './RemotePort.ts'
import {
  cannotRun,
  childStopped,
  describeEnding,
  endpointBlock,
  statusLine,
  statusState,
  WAITING_FOR_DEVICE,
} from './Status.ts'
import {
  BadCode,
  type Ending,
  PortNeverOpened,
  type SessionError,
} from './errors.ts'

/** The launcher's contract. A `--` child's own code is passed through instead. */
export const EXIT = {
  ended: 0,
  local: 1,
  badCodeOrToken: 2,
  relayUnreachable: 3,
  unsupportedVersion: 4,
  portNeverOpened: 5,
  childNotStarted: 127,
} as const

export const PORT_OPEN_WAIT_SECONDS = 30
const REDRAW = '250 millis'
/** How long a `--` child has to exit after SIGTERM before it gets SIGKILL. */
const CHILD_GRACE = '3 seconds'
const TOKEN_VARIABLE = 'CYCLING_REMOTE_TOKEN'

export const exitCodeOf = (error: SessionError): number =>
  Match.valueTags(error, {
    Unauthorized: () => EXIT.badCodeOrToken,
    SessionNotFound: () => EXIT.badCodeOrToken,
    BadCode: () => EXIT.badCodeOrToken,
    UnsupportedVersion: () => EXIT.unsupportedVersion,
    RelayUnreachable: () => EXIT.relayUnreachable,
    LocalResourceError: () => EXIT.local,
    PortNeverOpened: () => EXIT.portNeverOpened,
  })

const exitOf = (ending: Ending): number =>
  ending._tag === 'Rejected' ? exitCodeOf(ending.error) : EXIT.ended

const decodeCode = (input: string) =>
  Effect.fromOption(parseCode(input)).pipe(
    Effect.mapError(() => new BadCode({ input })),
  )

/**
 * The environment a `--` child runs with: the agent's own plus both
 * endpoints, minus every variable that carries the relay token.
 */
export const childEnvironment = (options: {
  readonly parent: Readonly<Record<string, string | undefined>>
  readonly token: string
  readonly endpoints: Readonly<Record<string, string>>
}): Record<string, string> => ({
  ...Object.fromEntries(
    Object.entries(options.parent).flatMap(([name, value]) =>
      value === undefined ||
      name === TOKEN_VARIABLE ||
      value.includes(options.token)
        ? []
        : [[name, value]],
    ),
  ),
  ...options.endpoints,
})

/** Foreground mode: one redrawn status line on a TTY, a plain line per change otherwise. Runs until interrupted. */
const watch = (port: RemotePort, endpoints: Bridge.Endpoints) =>
  Effect.gen(function* () {
    const tty = process.stdout.isTTY === true
    const printed = yield* Ref.make('')
    const draw = Effect.gen(function* () {
      const status = yield* SubscriptionRef.get(port.status)
      const ptyDropped = yield* endpoints.ptyDropped
      const line = statusLine({ status, ptyDropped })
      if (tty) {
        process.stdout.write(`\r\u001b[K${line}`)
        return
      }
      const state = statusState(status)
      if ((yield* Ref.getAndSet(printed, state)) !== state) {
        yield* Console.log(line)
      }
    })
    const finish = tty
      ? draw.pipe(Effect.andThen(Effect.sync(() => process.stdout.write('\n'))))
      : Effect.void
    return yield* Effect.forever(
      draw.pipe(Effect.andThen(Effect.sleep(REDRAW))),
    ).pipe(Effect.ensuring(Effect.asVoid(finish)))
  })

/** The child's exit code; it is killed when the scope closes first. */
const runCommand = (
  command: Arr.NonEmptyReadonlyArray<string>,
  env: Record<string, string>,
) =>
  Effect.gen(function* () {
    const [program, ...args] = command
    const spawner = yield* ChildProcessSpawner.ChildProcessSpawner
    return yield* spawner
      .spawn(
        ChildProcess.make(program, args, {
          env,
          stdin: 'inherit',
          stdout: 'inherit',
          stderr: 'inherit',
          detached: false,
          forceKillAfter: CHILD_GRACE,
        }),
      )
      .pipe(
        Effect.matchEffect({
          onFailure: error =>
            Console.error(cannotRun(program, error.message)).pipe(
              Effect.as(EXIT.childNotStarted),
            ),
          onSuccess: child =>
            child.exitCode.pipe(
              Effect.map(Number),
              Effect.catch(error =>
                Console.error(childStopped(program, error.message)).pipe(
                  Effect.as(EXIT.local),
                ),
              ),
            ),
        }),
        Effect.scoped,
      )
  })

/** `-- <cmd>` mode: the same bridge, one command with the endpoints in its environment, its exit code mirrored. */
const runChild = (options: {
  readonly port: RemotePort
  readonly endpoints: Bridge.Endpoints
  readonly command: Arr.NonEmptyReadonlyArray<string>
  readonly token: string
}) =>
  Effect.gen(function* () {
    const { port, endpoints } = options
    yield* firstMatch(port.status, status =>
      isPortOpen(status) ? Option.some(true) : Option.none(),
    ).pipe(
      Effect.timeoutOrElse({
        duration: `${PORT_OPEN_WAIT_SECONDS} seconds`,
        orElse: () =>
          Effect.gen(function* () {
            const status = yield* SubscriptionRef.get(port.status)
            return yield* new PortNeverOpened({
              seconds: PORT_OPEN_WAIT_SECONDS,
              state: statusState(status),
            })
          }),
      }),
    )
    return yield* runCommand(
      options.command,
      childEnvironment({
        parent: process.env,
        token: options.token,
        endpoints: { CYCLING_PORT: endpoints.pty },
      }),
    )
  })

const waitForDevice = (port: RemotePort) =>
  Effect.gen(function* () {
    const known = (yield* SubscriptionRef.get(port.status)).device
    if (Option.isNone(known)) {
      yield* Console.error(WAITING_FOR_DEVICE)
    }
    return yield* port.device
  })

type Outcome =
  | { readonly _tag: 'Finished'; readonly exit: number }
  | { readonly _tag: 'Over'; readonly ending: Ending }

/** Succeeds with the process exit code. */
export const runSession = (input: {
  readonly code: string
  readonly command: ReadonlyArray<string>
}): Effect.Effect<
  number,
  SessionError,
  | AgentConfig
  | Remote
  | PtyHost
  | FileSystem.FileSystem
  | ChildProcessSpawner.ChildProcessSpawner
> =>
  Effect.scoped(
    Effect.gen(function* () {
      const config = yield* AgentConfig
      const code: Code = yield* decodeCode(input.code)
      const port = yield* (yield* Remote).attach(code)
      const { command } = input
      const print = Arr.isReadonlyArrayNonEmpty(command)
        ? Console.error
        : Console.log

      const bridge = Effect.gen(function* () {
        const device: Device = yield* waitForDevice(port)
        if (device._tag === 'usb') {
          yield* Console.log(JSON.stringify(device.descriptor, null, 2))
          return EXIT.ended
        }
        const endpoints = yield* Bridge.open({ port, dir: config.dir, code })
        yield* print(
          endpointBlock({
            code,
            relay: config.relay,
            device,
            pty: endpoints.pty,
          }),
        )
        return Arr.isReadonlyArrayNonEmpty(command)
          ? yield* runChild({
              port,
              endpoints,
              command,
              token: Redacted.value(config.token),
            })
          : yield* watch(port, endpoints)
      })

      const outcome = yield* Effect.raceFirst(
        bridge.pipe(
          Effect.map((exit): Outcome => ({ _tag: 'Finished', exit })),
        ),
        over(port).pipe(
          Effect.map((ending): Outcome => ({ _tag: 'Over', ending })),
        ),
      )
      if (outcome._tag === 'Finished') return outcome.exit
      yield* print(describeEnding(outcome.ending))
      return exitOf(outcome.ending)
    }),
  )
