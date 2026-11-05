import * as Match from 'effect/Match'
import * as Option from 'effect/Option'
import type { Html } from 'foldkit/html'

import { type Region, formatBytes } from '@cycling/protocol'

import { labelOf, stakeOf } from './flasher/partitions.ts'
import { headState, layoutOf, runningWrite } from './holder.ts'
import { Message } from './message.ts'
import type { Activity, Layout, Live, Port, QueuedOp } from './model.ts'
import { type H, type Tone, button } from './parts.ts'

/**
 * Update mode as the visitor sees it: what the developer is doing to the
 * device in its own partition names, the consent card, the recent ops, and
 * the question Disconnect asks during a write.
 */

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

export const updatePanel = (live: Live, port: Port, h: H): Html => {
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

export const activityView = (activity: ReadonlyArray<Activity>, h: H): Html =>
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

export const stopGuard = (port: Port, region: Region, h: H): Html =>
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
