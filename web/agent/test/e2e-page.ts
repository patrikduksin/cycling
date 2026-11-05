import * as Effect from 'effect/Effect'
import * as Schedule from 'effect/Schedule'
import * as Schema from 'effect/Schema'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { launch, type Page } from './support/cdp.ts'

/**
 * The whole page flasher with no device: the built site in headless
 * Chromium, a fake Web Serial port backed by the ROM simulator injected
 * before the page loads, the relay, the real `cycling remote` launcher and
 * devtools' own Device through PageFlasher. Run by e2e-page.sh inside the
 * offline relay's network namespace:
 *
 *   bun web/agent/test/e2e-page.ts <relay-url>
 *
 * Env: AGENT_TOKEN, E2E_DIST (the built site), E2E_SIM (the simulator's
 * browser bundle), E2E_SHOTS (where screenshots go). Exits 0 when every
 * step holds.
 */

const RELAY = (process.argv[2] ?? '').replace(/^http/, 'ws').replace(/\/$/, '')
const AGENT_TOKEN = process.env['AGENT_TOKEN'] ?? ''
const DIST = process.env['E2E_DIST'] ?? ''
const SIM = process.env['E2E_SIM'] ?? ''
const SHOTS = process.env['E2E_SHOTS'] ?? ''
/** Set to a MiB count to time one read of that size instead of the flow and chaos runs. */
const BENCH_MIB = process.env['E2E_BENCH_MIB'] ?? ''
/** The simulated chip's READ_FLASH rate for the bench; the C606 manages about 140. */
const BENCH_KBPS = process.env['E2E_BENCH_KBPS'] ?? '140'
const ROOT = new URL('../../..', import.meta.url).pathname
const DEVICE_SCRIPT = new URL('./e2e_device.py', import.meta.url).pathname
const SITE_PORT = 5173

class E2eFailure extends Schema.TaggedError<
  E2eFailure,
  { readonly brand: unique symbol }
>()('E2eFailure', { message: Schema.String }) {}

const fail = (message: string) => Effect.fail(new E2eFailure({ message }))
const say = (line: string) =>
  Effect.sync(() => process.stdout.write(`${line}\n`))
const check = (label: string, ok: boolean, detail: unknown) =>
  ok ? say(`  ok   ${label}`) : fail(`${label}: ${JSON.stringify(detail)}`)

const inUpdateMode = (text: string) =>
  /your device is (in|entering) update mode/i.test(text)
const consoleBack = (text: string) =>
  !inUpdateMode(text) && /open at/i.test(text)

/** What the visitor reads, as the page renders it. */
const pageText = (page: Page) =>
  page
    .evaluate("document.body?.innerText ?? ''")
    .pipe(
      Effect.flatMap(Schema.decodeUnknownEffect(Schema.String)),
      Effect.orDie,
    )

const shoot = (page: Page, name: string) =>
  page.screenshot.pipe(
    Effect.flatMap(png =>
      Effect.promise(() => writeFile(join(SHOTS, `${name}.png`), png)),
    ),
    Effect.andThen(say(`  shot ${name}.png`)),
  )

const until = (
  page: Page,
  label: string,
  ok: (text: string) => boolean,
  seconds = 30,
) =>
  pageText(page).pipe(
    Effect.repeat({ until: ok, schedule: Schedule.spaced('200 millis') }),
    Effect.timeoutOrElse({
      duration: `${seconds} seconds`,
      orElse: () =>
        pageText(page).pipe(
          Effect.flatMap(text =>
            fail(`${label}: page says ${text.slice(0, 400)}`),
          ),
        ),
    }),
  )

const clickButton = (page: Page, text: string) =>
  page
    .evaluate(
      `(() => { const b = [...document.querySelectorAll('button')].find(b => b.innerText.trim().toLowerCase() === ${JSON.stringify(text.toLowerCase())}); if (!b) return false; b.click(); return true })()`,
    )
    .pipe(
      Effect.flatMap(clicked =>
        clicked === true
          ? Effect.void
          : fail(`no button "${text}" on the page`),
      ),
    )

const serveSite = Effect.acquireRelease(
  Effect.sync(() =>
    Bun.serve({
      port: SITE_PORT,
      hostname: 'localhost',
      fetch: async request => {
        const path = new URL(request.url).pathname
        const file = Bun.file(join(DIST, path === '/' ? 'index.html' : path))
        return (await file.exists())
          ? new Response(file)
          : new Response(Bun.file(join(DIST, 'index.html')))
      },
    }),
  ),
  server => Effect.promise(() => server.stop(true)),
)

/** `cycling remote <code> -- python e2e_device.py <mode> <scratch>`; resolves with its exit code and output. */
const remoteRun = (code: string, mode: string, scratch: string) =>
  Effect.acquireRelease(
    Effect.sync(() =>
      Bun.spawn(
        [
          'uv',
          'run',
          '--project',
          'tools/devtools',
          '--locked',
          'python',
          '-m',
          'cycling_devtools.cli',
          'remote',
          code,
          '--',
          'uv',
          'run',
          '--project',
          'tools/devtools',
          '--locked',
          'python',
          DEVICE_SCRIPT,
          mode,
          scratch,
        ],
        {
          cwd: ROOT,
          env: {
            ...process.env,
            CYCLING_REMOTE_TOKEN: AGENT_TOKEN,
            CYCLING_REMOTE_RELAY: RELAY,
          },
          stdout: 'pipe',
          stderr: 'pipe',
        },
      ),
    ),
    child => Effect.sync(() => child.kill()),
  ).pipe(
    Effect.map(child => ({
      done: Effect.promise(() =>
        Promise.all([
          child.exited,
          new Response(child.stdout).text(),
          new Response(child.stderr).text(),
        ]),
      ).pipe(
        Effect.map(([exit, stdout, stderr]) => ({ exit, stdout, stderr })),
      ),
    })),
  )

const scratchDir = Effect.promise(() => mkdtemp(join(tmpdir(), 'e2e-device-')))

const SimState = Schema.Struct({ mode: Schema.String })

const simState = (page: Page) =>
  page
    .evaluate('globalThis.__sim.state()')
    .pipe(Effect.flatMap(Schema.decodeUnknownEffect(SimState)), Effect.orDie)

const connect = (page: Page, query = '') =>
  Effect.gen(function* () {
    yield* page.navigate(`http://localhost:${SITE_PORT}/${query}`)
    yield* until(page, 'the page', text => text.length > 0)
    yield* page.evaluate('sessionStorage.clear()')
    yield* page.navigate(`http://localhost:${SITE_PORT}/${query}`)
    yield* until(page, 'the idle page', text => /connect device/i.test(text))
    yield* clickButton(page, 'Connect device')
    const text = yield* until(page, 'a session code', text =>
      /\b[0-9A-Z]{4}-[0-9A-Z]{4}\b/.test(text),
    )
    const code = /\b([0-9A-Z]{4}-[0-9A-Z]{4})\b/.exec(text)?.[1] ?? ''
    yield* say(`session ${code}`)
    return code
  })

const flow = (page: Page) =>
  Effect.scoped(
    Effect.gen(function* () {
      yield* say('flow: enter, read 0x8000, write+verify, readback, reboot')
      const code = yield* connect(page)
      const scratch = yield* scratchDir
      const run = yield* remoteRun(code, 'flow', scratch)
      yield* until(page, 'update mode', inUpdateMode, 60)
      yield* shoot(page, 'e2e-update-mode')
      yield* until(
        page,
        'the consent card',
        text => /wants to write/i.test(text),
        60,
      )
      yield* shoot(page, 'e2e-consent')
      yield* clickButton(page, 'Allow')
      yield* until(page, 'a write', text => /writing/i.test(text), 30)
      yield* shoot(page, 'e2e-write')
      const result = yield* run.done
      yield* say(result.stdout.trim().replace(/^/gm, '  py   '))
      yield* say(
        result.stderr
          .split('\n')
          .filter(line => line.startsWith('flasher:'))
          .join('\n')
          .replace(/^/gm, '  agt  '),
      )
      yield* check('the device task exited 0', result.exit === 0, result)
      yield* until(page, 'the console back', consoleBack, 30)
      yield* shoot(page, 'e2e-after-reboot')
      const state = yield* simState(page)
      yield* check(
        'the simulated chip runs its app',
        state.mode === 'app',
        state,
      )
      const after = yield* pageText(page)
      yield* check(
        'the MAC never reached the page',
        !/02[:-]?00[:-]?00/i.test(after),
        after,
      )
    }),
  )

const chaos = (page: Page) =>
  Effect.scoped(
    Effect.gen(function* () {
      yield* say('chaos: kill the device task mid-read')
      const code = yield* connect(page)
      const scratch = yield* scratchDir
      const run = yield* remoteRun(code, 'long-read', scratch)
      yield* until(
        page,
        'a read under way',
        text => /reading .*: [1-9]\d*%/i.test(text),
        90,
      )
      yield* shoot(page, 'e2e-mid-read')
      const pid = Number(
        yield* Effect.promise(() => Bun.file(join(scratch, 'pid')).text()),
      )
      process.kill(pid, 'SIGKILL')
      yield* say(`  killed the device task (pid ${pid})`)
      const result = yield* run.done
      yield* check(
        'cycling remote mirrors the killed child',
        result.exit !== 0,
        result.exit,
      )
      yield* until(page, 'the console back after Release', consoleBack, 30)
      yield* shoot(page, 'e2e-after-kill')
      const state = yield* simState(page)
      yield* check(
        'the released chip is back in its app, not download mode',
        state.mode === 'app',
        state,
      )
    }),
  )

const bench = (page: Page) =>
  Effect.scoped(
    Effect.gen(function* () {
      yield* say(
        `bench: read ${BENCH_MIB} MiB from a chip at ${BENCH_KBPS} KB/s`,
      )
      const code = yield* connect(page, `?sim-read-kbps=${BENCH_KBPS}`)
      const scratch = yield* scratchDir
      const run = yield* remoteRun(code, 'bench', scratch)
      const result = yield* run.done
      yield* say(result.stdout.trim().replace(/^/gm, '  py   '))
      yield* check('the bench read finished', result.exit === 0, result)
    }),
  )

const program = Effect.scoped(
  Effect.gen(function* () {
    if (RELAY === '' || DIST === '' || SIM === '' || SHOTS === '') {
      return yield* fail(
        'usage: E2E_DIST=... E2E_SIM=... E2E_SHOTS=... bun web/agent/test/e2e-page.ts <relay-url>',
      )
    }
    yield* serveSite
    const page = yield* launch({
      preload: SIM,
      width: 900,
      height: 900,
      dark: true,
    })
    if (BENCH_MIB !== '') {
      return yield* bench(page)
    }
    yield* flow(page)
    yield* chaos(page)
    yield* say('all steps held')
  }),
)

void Effect.runPromiseExit(program).then(exit => {
  if (exit._tag === 'Failure') {
    process.stderr.write(`FAILED: ${String(exit.cause)}\n`)
    process.exit(1)
  }
  process.exit(0)
})
