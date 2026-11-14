import { memo } from 'react'
import type { MessageKey } from '../../../shared/i18n/messages'
import type {
  ThreadItem,
  TurnAttachmentDelivery,
  TurnAttachmentOutcome
} from '../../../shared/contracts'
import { formatClock } from '../../../shared/format'
import { useI18n } from '../../../shared/i18n'
import { Markdown } from '../../../shared/ui/Markdown'
import { CopyButton } from '../../../shared/ui/copy'

/*
 * Rows are memoized on their `item` object. The reducer keeps the identity of
 * every item a fold does not touch (`items-draft.ts`), so one streamed token
 * re-renders exactly one row instead of the whole turn.
 */

/** Delivery-state chip color. `image` and `preview_text` are healthy paths. */
function attachmentChipClass(delivered: TurnAttachmentDelivery): string {
  if (delivered === 'image' || delivered === 'preview_text') return 'ark-chip ark-chip--muted'
  return 'ark-chip ark-chip--warn'
}

/** Explicit delivery → label mapping: the wire enum is snake_case, the catalog is not. */
function attachmentLabelKey(delivered: TurnAttachmentDelivery): MessageKey {
  switch (delivered) {
    case 'image':
      return 'canvas.attachments.image'
    case 'preview_text':
      return 'canvas.attachments.previewText'
    case 'degraded':
      return 'canvas.attachments.degraded'
    case 'skipped':
      return 'canvas.attachments.skipped'
    case 'unavailable':
      return 'canvas.attachments.unavailable'
    default: {
      // Exhaustiveness guard: a new wire variant must land here with a label.
      const unhandled: never = delivered
      throw new Error(`unmapped attachment delivery: ${String(unhandled)}`)
    }
  }
}

/** One attachment chip per outcome, so a user can see what the model received. */
export function AttachmentChips({ outcomes }: { outcomes: TurnAttachmentOutcome[] }): JSX.Element {
  const { t } = useI18n()
  return (
    <div className="ark-msg__attachments" aria-label={t('canvas.attachments.title')}>
      {outcomes.map((outcome) => (
        <span
          key={outcome.attachment_id}
          className={attachmentChipClass(outcome.delivered)}
          title={outcome.detail ?? undefined}
        >
          {outcome.original_filename ?? outcome.attachment_id} · {t(attachmentLabelKey(outcome.delivered))}
        </span>
      ))}
    </div>
  )
}

/** User message bubble. Text is the runtime's accumulated `text`, verbatim. */
export const UserBubble = memo(function UserBubble({ item }: { item: ThreadItem }): JSX.Element {
  const { t, locale } = useI18n()
  const text = item.text ?? ''
  return (
    <article className="ark-msg ark-msg--user">
      <header className="ark-msg__head">
        <span className="ark-msg__avatar ark-msg__avatar--you" aria-hidden="true">
          {t('canvas.author.youInitial')}
        </span>
        <span className="ark-msg__author">{t('canvas.author.you')}</span>
        <time className="ark-msg__time">{formatClock(item.created_at, locale)}</time>
        <CopyButton
          text={text}
          label={t('canvas.copy.message')}
          copiedLabel={t('canvas.copy.copied')}
          className="ark-msg__copy"
        />
      </header>
      <div className="ark-msg__body">{text}</div>
      {item.attachments && item.attachments.length > 0 ? (
        <AttachmentChips outcomes={item.attachments} />
      ) : null}
    </article>
  )
})

/**
 * Assistant bubble. `streaming` only drives the caret: the text itself is never
 * re-parsed or re-assembled on the client. The copy button appears once the
 * turn stops streaming — mid-stream the text is still growing, so a snapshot
 * copy would mislead.
 */
export const AssistantBubble = memo(function AssistantBubble({ item }: { item: ThreadItem }): JSX.Element {
  const { t, locale } = useI18n()
  const streaming = item.status === 'streaming' || item.status === 'pending'
  const failed = item.status === 'failed'
  const aborted = item.status === 'aborted'
  const text = item.text ?? ''

  return (
    <article className={`ark-msg ark-msg--assistant${failed ? ' is-failed' : ''}`}>
      <header className="ark-msg__head">
        <span className="ark-msg__avatar ark-msg__avatar--assistant" aria-hidden="true">
          ark
        </span>
        <span className="ark-msg__author">ark</span>
        <time className="ark-msg__time">{formatClock(item.created_at, locale)}</time>
        {aborted ? <span className="ark-chip ark-chip--muted">{t('canvas.aborted')}</span> : null}
        {failed ? <span className="ark-chip ark-chip--error">{t('canvas.failed')}</span> : null}
        {!streaming && text ? (
          <CopyButton
            text={text}
            label={t('canvas.copy.message')}
            copiedLabel={t('canvas.copy.copied')}
            className="ark-msg__copy"
          />
        ) : null}
      </header>
      <div className="ark-msg__body ark-msg__body--prose">
        <Markdown text={text} />
        {streaming ? <span className="ark-caret" aria-label={t('canvas.generating')} /> : null}
      </div>
      {item.detail ? <p className="ark-msg__detail">{item.detail}</p> : null}
    </article>
  )
})

/** Reasoning is collapsed by default and visually subordinate. */
export const ReasoningBlock = memo(function ReasoningBlock({ item }: { item: ThreadItem }): JSX.Element {
  const { t, locale } = useI18n()
  return (
    <details className="ark-reasoning">
      <summary className="ark-reasoning__summary">
        {t('canvas.reasoning')}
        <span className="ark-reasoning__meta">{formatClock(item.created_at, locale)}</span>
      </summary>
      <pre className="ark-reasoning__body">{item.text ?? ''}</pre>
    </details>
  )
})
