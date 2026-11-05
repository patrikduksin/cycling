import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import type { Document, Html, HtmlBuilder } from 'foldkit/html'

import {
  type UsbDescriptor,
  formatBytes,
  formatCode,
  formatUsbId,
} from '@cycling/protocol'

import { usbClassName } from './adapters/usb.ts'
import { Message } from './message.ts'
import {
  type Ending,
  type Live,
  type Missing,
  type Model,
  type Monitor,
  type Port,
  Page,
  Relay,
  Source,
} from './model.ts'

type H = HtmlBuilder<Message>

const button = (
  h: H,
  options: {
    readonly label: string
    readonly onClick: Message
    readonly tone?: 'primary' | 'danger' | 'quiet'
    readonly disabled?: boolean
  },
): Html =>
  h.button(
    [
      h.Class(`button ${options.tone ?? 'primary'}`),
      h.Type('button'),
      h.Disabled(options.disabled ?? false),
      h.OnClick(options.onClick),
    ],
    [options.label],
  )

const shortScreen = (
  h: H,
  content: {
    readonly title: string
    readonly lede: string
    readonly actions: ReadonlyArray<Html>
    readonly notice?: string | undefined
  },
): Html =>
  h.section(
    [h.Class('screen short')],
    [
      h.div(
        [h.Class('heading')],
        [h.h1([], [content.title]), h.p([h.Class('lede')], [content.lede])],
      ),
      content.actions.length === 0
        ? null
        : h.div([h.Class('actions')], [...content.actions]),
      content.notice === undefined
        ? null
        : h.p([h.Class('notice'), h.Role('alert')], [content.notice]),
    ],
  )

const MISSING_TEXT: Readonly<
  Record<Missing, { readonly title: string; readonly lede: string }>
> = {
  secureContext: {
    title: 'Open in Chrome or Edge',
    lede: 'Sharing a device needs https.',
  },
  webSerial: {
    title: 'Open in Chrome or Edge',
    lede: 'This browser cannot share a USB device.',
  },
  relayUrl: {
    title: 'Not configured',
    lede: 'This build has no relay address (VITE_RELAY_URL).',
  },
}

const unsupportedView = (missing: Missing, h: H): Html =>
  shortScreen(h, { ...MISSING_TEXT[missing], actions: [] })

const idleView = (
  model: Model,
  notice: string | undefined,
  busy: boolean,
  h: H,
): Html =>
  shortScreen(h, {
    title: 'Share a device',
    lede: 'Plug it in, connect, and read the code to the developer.',
    actions: [
      button(h, {
        label: 'Connect device',
        onClick: Message.ClickedConnectDevice(),
        disabled: busy,
      }),
      ...(model.canDescribeUsb
        ? [
            button(h, {
              label: 'Not a serial port?',
              onClick: Message.ClickedDescribeUsb(),
              tone: 'quiet',
              disabled: busy,
            }),
          ]
        : []),
    ],
    notice,
  })

const ENDING_TEXT: Readonly<Record<Ending, string>> = {
  visitorDisconnected: 'You disconnected. The developer has no access now.',
  visitorGone: 'This page was offline too long.',
  expired: 'The session timed out.',
  replaced: 'Another tab took over this session.',
  unauthorized: 'The relay refused this session.',
  notFound: 'The relay refused this session.',
  unsupportedVersion: 'This page is out of date. Reload it.',
  rateLimited: 'Too many sessions from this network. Try again in a minute.',
  protocolViolation:
    'The relay and this page disagreed, so the session closed.',
}

const endedView = (reason: Ending, h: H): Html =>
  shortScreen(h, {
    title: 'Session ended',
    lede: ENDING_TEXT[reason],
    actions: [
      button(h, { label: 'Start over', onClick: Message.ClickedStartOver() }),
    ],
  })

type Tone = 'good' | 'attention' | 'bad' | 'plain'

interface Status {
  readonly label: string
  readonly value: string
  readonly tone: Tone
}

const RELAY_STATUS: Readonly<
  Record<Relay['_tag'], { readonly value: string; readonly tone: Tone }>
> = {
  Dialing: { value: 'connecting', tone: 'attention' },
  Joined: { value: 'connected', tone: 'good' },
  Backoff: { value: 'reconnecting', tone: 'attention' },
  Leaving: { value: 'disconnecting', tone: 'attention' },
}

const relayStatus = (relay: Relay): Status => ({
  label: 'Relay',
  ...RELAY_STATUS[relay._tag],
})

const developerStatus = (relay: Relay): Status =>
  Relay.guards.Joined(relay) && relay.agentPresent
    ? { label: 'Developer', value: 'connected', tone: 'good' }
    : { label: 'Developer', value: 'waiting', tone: 'attention' }

const portStatus = (port: Port): Status => ({
  label: 'Port',
  ...Match.value(port.state).pipe(
    Match.withReturnType<{ readonly value: string; readonly tone: Tone }>(),
    Match.tagsExhaustive({
      Open: ({ baud }) => ({ value: `open at ${baud}`, tone: 'good' }),
      Opening: () => ({ value: 'opening', tone: 'attention' }),
      Reacquiring: () => ({ value: 'reconnecting', tone: 'attention' }),
      Failed: ({ reason }) => ({
        value:
          reason === 'reacquireTimeout'
            ? 'did not come back'
            : 'in use by another program or tab',
        tone: 'bad',
      }),
    }),
  ),
})

const statusList = (rows: ReadonlyArray<Status>, h: H): Html =>
  h.dl(
    [h.Class('status'), h.AriaLive('polite')],
    rows.map(row =>
      h.div(
        [h.Class('status-line')],
        [h.dt([], [row.label]), h.dd([h.Class(row.tone)], [row.value])],
      ),
    ),
  )

const hero = (live: Live, h: H): Html => {
  const code = formatCode(live.session.code)
  return h.div(
    [h.Class('hero')],
    [
      h.p([h.Class('eyebrow')], ['Read this to the developer']),
      h.p([h.Class('code'), h.AriaLabel('Session code')], [code]),
      button(h, {
        label: live.copied ? 'Copied' : 'Copy code',
        onClick: Message.ClickedCopyCode(),
        tone: 'quiet',
      }),
    ],
  )
}

const count = (label: string, bytes: number, tone: string, h: H): Html =>
  h.div(
    [h.Class(`count ${tone}`)],
    [
      h.span([h.Class('caption')], [label]),
      h.span([h.Class('number')], [formatBytes(bytes)]),
    ],
  )

const monitorView = (monitor: Monitor, h: H): Html => {
  const dropped = monitor.droppedFromDevice + monitor.droppedToDevice
  return h.div(
    [h.Class('monitor-block')],
    [
      h.div(
        [h.Class('counts')],
        [
          count('In', monitor.bytesFromDevice, 'from-device', h),
          count('Out', monitor.bytesToDevice, 'to-device', h),
          dropped > 0
            ? count('Dropped', dropped, 'dropped', h)
            : h.div([h.Class('count')], []),
        ],
      ),
      h.div(
        [h.Class('monitor'), h.Role('log'), h.AriaLive('off')],
        monitor.rows.length === 0
          ? [h.p([h.Class('empty')], ['No bytes yet.'])]
          : [...monitor.rows]
              .reverse()
              .map(row =>
                h.keyed('div')(
                  row.id,
                  [
                    h.Class(
                      `row ${row.direction === 'fromDevice' ? 'from-device' : 'to-device'}`,
                    ),
                  ],
                  [
                    h.span(
                      [h.Class('dir')],
                      [row.direction === 'fromDevice' ? 'IN' : 'OUT'],
                    ),
                    h.span([h.Class('hex')], [row.hex]),
                    h.span([h.Class('text')], [row.text]),
                    row.hiddenBytes > 0
                      ? h.span(
                          [h.Class('more')],
                          [`+${row.hiddenBytes} more bytes`],
                        )
                      : null,
                  ],
                ),
              ),
      ),
    ],
  )
}

const disconnect = (live: Live, h: H): Html =>
  h.div(
    [h.Class('actions')],
    [
      button(h, {
        label: 'Disconnect',
        onClick: Message.ClickedDisconnect(),
        tone: 'danger',
        disabled: Relay.guards.Leaving(live.relay),
      }),
    ],
  )

const serialLiveView = (live: Live, port: Port, h: H): Html =>
  h.section(
    [h.Class('screen live')],
    [
      hero(live, h),
      statusList(
        [
          {
            label: 'Device',
            value: `USB ${formatUsbId(port.identity)}`,
            tone: 'plain',
          },
          relayStatus(live.relay),
          developerStatus(live.relay),
          portStatus(port),
        ],
        h,
      ),
      port.state._tag === 'Failed'
        ? h.div(
            [h.Class('actions')],
            [
              button(h, {
                label: 'Choose device again',
                onClick: Message.ClickedChooseAgain(),
              }),
            ],
          )
        : null,
      monitorView(live.monitor, h),
      disconnect(live, h),
    ],
  )

const usbDevice = (descriptor: UsbDescriptor): string =>
  `${formatUsbId(descriptor)}, ${usbClassName(descriptor.deviceClass)}`

const usbLiveView = (live: Live, descriptor: UsbDescriptor, h: H): Html =>
  h.section(
    [h.Class('screen live')],
    [
      hero(live, h),
      statusList(
        [
          { label: 'Device', value: usbDevice(descriptor), tone: 'plain' },
          relayStatus(live.relay),
          developerStatus(live.relay),
        ],
        h,
      ),
      h.p(
        [h.Class('sent'), h.AriaLive('polite')],
        [
          Relay.guards.Joined(live.relay)
            ? 'The developer can see this device’s description. No data flows.'
            : 'Connecting to the relay.',
        ],
      ),
      disconnect(live, h),
    ],
  )

const pageView = (model: Model, h: H): Html =>
  Page.match(model.page, {
    Unsupported: ({ missing }) => unsupportedView(missing, h),
    Idle: ({ notice }) =>
      idleView(model, Option.getOrUndefined(notice), false, h),
    Choosing: () => idleView(model, undefined, true, h),
    Live: live =>
      Source.match(live.source, {
        Serial: ({ port }) => serialLiveView(live, port, h),
        Usb: ({ descriptor }) => usbLiveView(live, descriptor, h),
      }),
    Ended: ({ reason }) => endedView(reason, h),
  })

export const view = (model: Model, h: H): Document => ({
  title: 'Share a USB device',
  body: h.main([h.Class('shell')], [pageView(model, h)]),
})
