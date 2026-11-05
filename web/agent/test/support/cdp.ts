import * as Effect from 'effect/Effect'
import * as Schema from 'effect/Schema'
import type * as Scope from 'effect/Scope'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

/**
 * Just enough of the Chrome DevTools Protocol to drive one headless page:
 * evaluate, inject a script before the page's own, set the colour scheme,
 * take screenshots. No dependency beyond the chromium on PATH.
 */
export interface Page {
  /** The expression's value, as JSON would carry it. */
  readonly evaluate: (expression: string) => Effect.Effect<unknown>
  readonly navigate: (url: string) => Effect.Effect<void>
  readonly screenshot: Effect.Effect<Uint8Array>
}

const Targets = Schema.Array(
  Schema.Struct({ type: Schema.String, webSocketDebuggerUrl: Schema.String }),
)

interface Reply {
  readonly id?: number
  readonly result?: Record<string, unknown>
  readonly error?: { readonly message: string }
}

const devtoolsUrl = (stderr: ReadableStream<Uint8Array>) =>
  Effect.promise(async () => {
    const decoder = new TextDecoder()
    let seen = ''
    for await (const chunk of stderr) {
      seen += decoder.decode(chunk)
      const found = /DevTools listening on (ws:\/\/\S+)/.exec(seen)
      if (found?.[1] !== undefined) return found[1]
    }
    throw new Error(`chromium exited before DevTools was up:\n${seen}`)
  })

export const launch = (options: {
  readonly preload: string
  readonly width: number
  readonly height: number
  readonly dark: boolean
}): Effect.Effect<Page, never, Scope.Scope> =>
  Effect.gen(function* () {
    const profile = yield* Effect.promise(() =>
      mkdtemp(join(tmpdir(), 'e2e-chromium-')),
    )
    const chromium = yield* Effect.acquireRelease(
      Effect.sync(() =>
        Bun.spawn(
          [
            'chromium',
            '--headless=new',
            '--no-sandbox',
            '--no-first-run',
            '--disable-gpu',
            '--remote-debugging-port=0',
            `--user-data-dir=${profile}`,
            `--window-size=${options.width},${options.height}`,
            'about:blank',
          ],
          { stdout: 'ignore', stderr: 'pipe' },
        ),
      ),
      child => Effect.sync(() => child.kill()),
    )
    const browserUrl = yield* devtoolsUrl(chromium.stderr)
    const http = browserUrl.replace(/^ws/, 'http').replace(/\/devtools.*$/, '')
    const targets = yield* Effect.promise(() =>
      fetch(`${http}/json/list`).then(response => response.json()),
    ).pipe(Effect.flatMap(Schema.decodeUnknownEffect(Targets)), Effect.orDie)
    const target = targets.find(entry => entry.type === 'page')
    if (target === undefined) return yield* Effect.die('no page target')
    const socket = yield* Effect.acquireRelease(
      Effect.promise(
        () =>
          new Promise<WebSocket>((resolve, reject) => {
            const ws = new WebSocket(target.webSocketDebuggerUrl)
            ws.onopen = () => resolve(ws)
            ws.onerror = () => reject(new Error('CDP socket failed'))
          }),
      ),
      ws => Effect.sync(() => ws.close()),
    )
    let nextId = 0
    const waiting = new Map<number, (reply: Reply) => void>()
    socket.onmessage = event => {
      const reply: Reply = JSON.parse(String(event.data))
      if (reply.id !== undefined) waiting.get(reply.id)?.(reply)
    }
    const send = (method: string, params: object = {}) =>
      Effect.promise(
        () =>
          new Promise<Record<string, unknown>>((resolve, reject) => {
            nextId += 1
            waiting.set(nextId, reply =>
              reply.error === undefined
                ? resolve(reply.result ?? {})
                : reject(new Error(`${method}: ${reply.error.message}`)),
            )
            socket.send(JSON.stringify({ id: nextId, method, params }))
          }),
      )

    yield* send('Page.enable')
    yield* send('Runtime.enable')
    yield* send('Emulation.setDeviceMetricsOverride', {
      width: options.width,
      height: options.height,
      deviceScaleFactor: 1,
      mobile: options.width < 600,
    })
    yield* send('Emulation.setEmulatedMedia', {
      features: [
        {
          name: 'prefers-color-scheme',
          value: options.dark ? 'dark' : 'light',
        },
      ],
    })
    const preload = yield* Effect.promise(() =>
      Bun.file(options.preload).text(),
    )
    yield* send('Page.addScriptToEvaluateOnNewDocument', { source: preload })

    return {
      evaluate: expression =>
        send('Runtime.evaluate', {
          expression,
          awaitPromise: true,
          returnByValue: true,
          userGesture: true,
        }).pipe(
          Effect.flatMap(result => {
            const thrown = result['exceptionDetails']
            if (thrown !== undefined) {
              return Effect.die(`page threw: ${JSON.stringify(thrown)}`)
            }
            const value = result['result']
            return Effect.succeed(
              typeof value === 'object' && value !== null && 'value' in value
                ? value.value
                : undefined,
            )
          }),
        ),
      navigate: url => send('Page.navigate', { url }).pipe(Effect.asVoid),
      screenshot: send('Page.captureScreenshot', { format: 'png' }).pipe(
        Effect.map(({ data }) =>
          Uint8Array.from(Buffer.from(String(data), 'base64')),
        ),
      ),
    }
  })
