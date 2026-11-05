import * as Console from 'effect/Console'
import * as Context from 'effect/Context'
import * as Effect from 'effect/Effect'
import * as Exit from 'effect/Exit'
import * as Layer from 'effect/Layer'
import * as Ref from 'effect/Ref'
import * as Runtime from 'effect/Runtime'
import * as Argument from 'effect/cli/Argument'
import * as Command from 'effect/cli/Command'

import * as BunRuntime from '@effect/platform-bun/BunRuntime'
import * as BunServices from '@effect/platform-bun/BunServices'

import { EXIT, exitCodeOf, runSession } from './Agent.ts'
import * as AgentConfig from './AgentConfig.ts'
import { BunTerminalPtyHost } from './PtyHost.ts'
import * as RelayClient from './RelayClient.ts'
import * as Remote from './Remote.ts'
import { describeConfig, describeError } from './Status.ts'
import type { SessionError } from './errors.ts'

/** The handlers' exit code; effect/cli handlers return void, so it is carried out here. */
class Exited extends Context.Service<Exited, Ref.Ref<number>>()(
  'agent/Exited',
) {}

const setExit = (code: number) =>
  Effect.gen(function* () {
    yield* Ref.set(yield* Exited, code)
  })

const live = Layer.mergeAll(
  Remote.layer.pipe(Layer.provide(RelayClient.layer)),
  BunTerminalPtyHost,
).pipe(Layer.provideMerge(AgentConfig.fromEnv))

/** The environment is read inside the handler, so --help works without it. */
const report = (
  program: Effect.Effect<
    number,
    SessionError,
    Layer.Success<typeof live> | BunServices.BunServices
  >,
) =>
  program.pipe(
    Effect.catch(error =>
      Console.error(describeError(error)).pipe(Effect.as(exitCodeOf(error))),
    ),
    Effect.provide(live),
    Effect.catchTag('ConfigError', error =>
      Console.error(describeConfig(error.message)).pipe(
        Effect.as(EXIT.badCodeOrToken),
      ),
    ),
    Effect.flatMap(setExit),
  )

const remote = Command.make(
  'remote',
  {
    code: Argument.String('code'),
    command: Argument.String('command').pipe(Argument.variadic()),
  },
  input => report(runSession(input)),
)

const cli = remote

const program = Effect.gen(function* () {
  yield* Command.run(cli, { version: '1' })
  return yield* Ref.get(yield* Exited)
}).pipe(
  Effect.provide(
    Layer.mergeAll(BunServices.layer, Layer.effect(Exited, Ref.make(0))),
  ),
)

const teardown: Runtime.Teardown = (exit, onExit) =>
  Exit.isSuccess(exit) && typeof exit.value === 'number'
    ? onExit(exit.value)
    : Runtime.defaultTeardown(exit, onExit)

BunRuntime.runMain(program, { teardown })
