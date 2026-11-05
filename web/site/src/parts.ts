import type { Html, HtmlBuilder } from 'foldkit/html'

import type { Message } from './message.ts'

export type H = HtmlBuilder<Message>

/** What a status or activity line's colour says: VANA's state colours, or none. */
export type Tone = 'good' | 'attention' | 'bad' | 'plain'

export const button = (
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
