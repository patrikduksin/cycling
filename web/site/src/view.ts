import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import type { Document, Html, HtmlBuilder } from 'foldkit/html'

import {
  type Region,
  type UsbDescriptor,
  formatBytes,
  formatCode,
  formatUsbId,
} from '@cycling/protocol'

import { usbClassName } from './adapters/usb.ts'
import { labelOf, stakeOf } from './flasher/partitions.ts'
import { headState, isHold, layoutOf, runningWrite } from './holder.ts'
import { Message } from './message.ts'
import {
  type Activity,
  type Ending,
  type Layout,
  type Live,
  type Missing,
  type Model,
  type Monitor,
  type Port,
  type QueuedOp,
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
      Entering: () => ({ value: 'entering update mode', tone: 'attention' }),
      Flashing: () => ({ value: 'update mode', tone: 'attention' }),
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

const hex = (address: number): string => `0x${address.toString(16)}`

/** "ota_1 (1.9 MiB at 0x760000)", or "1.9 MiB at 0x760000" where the device's table has no name for it. */
const place = (layout: Option.Option<Layout>, region: Region): string => {
  const where = `${formatBytes(region.size)} at ${hex(region.address)}`
  return Option.match(labelOf(layout, region), {
    onNone: () => where,
    onSome: label => `${label} (${where})`,
  })
}

const percent = (op: QueuedOp): string =>
  `${op.total === 0 ? 0 : Math.floor((op.done * 100) / op.total)}%`

/** What is happening to the device now, in the device's own words. */
const nowDoing = (
  port: Port,
  head: Option.Option<QueuedOp>,
  writesAllowed: boolean,
): string => {
  const layout = layoutOf(port)
  return Option.match(head, {
    onNone: () => 'Waiting for the developer.',
    onSome: running =>
      Match.value(running.op).pipe(
        Match.tagsExhaustive({
          Enter: () => 'The developer is connecting to it.',
          Read: ({ region }) =>
            `The developer is reading ${place(layout, region)}: ${percent(running)}`,
          Write: ({ region }) =>
            writesAllowed
              ? `The developer is writing ${place(layout, region)}: ${percent(running)}`
              : 'The developer is waiting for your answer.',
          Digest: ({ region }) =>
            `The developer is checking ${place(layout, region)}.`,
          Reboot: () => 'The developer is restarting your device.',
        }),
      ),
  })
}

const consentCard = (port: Port, region: Region, h: H): Html =>
  h.div(
    [h.Class('ask'), h.Role('alertdialog'), h.AriaLabel('Allow writing')],
    [
      h.p(
        [h.Class('question')],
        ['A developer wants to write to your device’s memory.'],
      ),
      h.p([h.Class('sent')], [`${place(layoutOf(port), region)}.`]),
      h.div(
        [h.Class('actions')],
        [
          button(h, { label: 'Allow', onClick: Message.ClickedAllowWrites() }),
          button(h, {
            label: 'Not now',
            onClick: Message.ClickedDeclineWrite(),
            tone: 'quiet',
          }),
        ],
      ),
    ],
  )

const updatePanel = (live: Live, port: Port, h: H): Html => {
  const state = headState(port, live.writesAllowed)
  const writing = Option.isSome(runningWrite(port, live.writesAllowed))
  const head = Match.value(state).pipe(
    Match.tag('Entering', ({ head }) => head),
    Match.tag('Running', 'AwaitingConsent', ({ head }) => Option.some(head)),
    Match.orElse(() => Option.none<QueuedOp>()),
  )
  return h.div(
    [h.Class('update'), h.AriaLive('polite')],
    [
      h.p([h.Class('eyebrow')], ['Update mode']),
      h.p(
        [h.Class('update-lead')],
        [
          port.state._tag === 'Entering'
            ? 'Your device is entering update mode.'
            : 'Your device is in update mode.',
        ],
      ),
      h.p(
        [h.Class(writing ? 'update-now writing' : 'update-now')],
        [nowDoing(port, head, live.writesAllowed)],
      ),
      h.p([h.Class('sent')], ['Keep this tab open and in front.']),
      state._tag === 'AwaitingConsent'
        ? consentCard(port, state.region, h)
        : null,
      state._tag === 'Idle'
        ? h.div(
            [h.Class('actions')],
            [
              button(h, {
                label: 'Restart device',
                onClick: Message.ClickedRestartDevice(),
              }),
            ],
          )
        : null,
    ],
  )
}

const KIND_TEXT: Readonly<Record<Activity['kind'], string>> = {
  Enter: 'Enter',
  Read: 'Read',
  Write: 'Write',
  Digest: 'Check',
  Reboot: 'Restart',
  Restart: 'Restart',
}

const RESULT_TEXT: Readonly<
  Record<Activity['result'], { readonly text: string; readonly tone: Tone }>
> = {
  done: { text: 'done', tone: 'plain' },
  failed: { text: 'failed', tone: 'bad' },
  stopped: { text: 'stopped', tone: 'attention' },
  declined: { text: 'not allowed', tone: 'attention' },
}

const activityPlace = (entry: Activity): string =>
  Option.match(entry.region, {
    onNone: () =>
      Match.value(entry.kind).pipe(
        Match.when('Restart', () => 'by you'),
        Match.when('Reboot', () => 'by the developer'),
        Match.orElse(() => 'update mode'),
      ),
    onSome: region => Option.getOrElse(entry.label, () => hex(region.address)),
  })

const activityView = (activity: ReadonlyArray<Activity>, h: H): Html =>
  h.div(
    [h.Class('activity-block')],
    [
      h.p([h.Class('eyebrow')], ['Device activity']),
      h.ul(
        [h.Class('activity'), h.AriaLive('off')],
        activity.map(entry =>
          h.keyed('li')(
            entry.seq,
            [
              h.Class(
                `activity-row ${entry.kind === 'Write' ? 'write' : 'quiet'}`,
              ),
            ],
            [
              h.span([h.Class('kind')], [KIND_TEXT[entry.kind]]),
              h.span([h.Class('place')], [activityPlace(entry)]),
              h.span(
                [h.Class('size')],
                [
                  Option.match(entry.region, {
                    onNone: () => '',
                    onSome: region => formatBytes(region.size),
                  }),
                ],
              ),
              h.span(
                [h.Class(`result ${RESULT_TEXT[entry.result].tone}`)],
                [RESULT_TEXT[entry.result].text],
              ),
            ],
          ),
        ),
      ),
    ],
  )

/** What stopping a write now would leave behind, checked against the device's own tables. */
const stopCopy = (port: Port, region: Region): string => {
  const layout = layoutOf(port)
  const breaks = 'Stopping now can leave your device unable to start.'
  return Match.value(stakeOf(layout, region)).pipe(
    Match.tagsExhaustive({
      Bootloader: () => `The developer is writing the bootloader. ${breaks}`,
      PartitionTable: () =>
        `The developer is writing the partition table. ${breaks}`,
      Otadata: () =>
        `The developer is writing otadata, which picks the firmware your device starts. ${breaks}`,
      BootSlot: ({ label }) =>
        `The developer is writing ${label}, the firmware your device starts from. ${breaks}`,
      Elsewhere: () => {
        const label = labelOf(layout, region)
        const isApp = Option.exists(label, name =>
          Option.exists(layout, ({ partitions }) =>
            partitions.some(entry => entry.label === name && entry.type === 0),
          ),
        )
        const name = Option.getOrElse(label, () => place(layout, region))
        return `The developer is writing ${name}. Stopping now leaves ${isApp ? 'that slot' : 'it'} incomplete; the firmware your device starts from is not being changed.`
      },
      Unknown: () =>
        `The developer is writing ${place(layout, region)}. Stopping now leaves it incomplete.`,
    }),
  )
}

const stopGuard = (port: Port, region: Region, h: H): Html =>
  h.div(
    [h.Class('ask'), h.Role('alertdialog'), h.AriaLabel('Stop writing')],
    [
      h.p([h.Class('question')], [stopCopy(port, region)]),
      h.div(
        [h.Class('actions')],
        [
          button(h, {
            label: 'Stop and disconnect',
            onClick: Message.ClickedStopAndDisconnect(),
            tone: 'danger',
          }),
          button(h, {
            label: 'Keep going',
            onClick: Message.ClickedKeepGoing(),
          }),
        ],
      ),
    ],
  )

const serialLiveView = (live: Live, port: Port, h: H): Html => {
  const guarded = runningWrite(port, live.writesAllowed).pipe(
    Option.filter(() => live.confirmingStop),
  )
  return h.section(
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
      isHold(port.state)
        ? updatePanel(live, port, h)
        : monitorView(live.monitor, h),
      port.activity.length === 0 ? null : activityView(port.activity, h),
      Option.match(guarded, {
        onNone: () => disconnect(live, h),
        onSome: ({ region }) => stopGuard(port, region, h),
      }),
    ],
  )
}

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
