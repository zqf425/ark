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
import { useThreadSkills } from '../hooks/use-thread-skills'
import { splitSkillMentions } from './skill-mention'

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

/**
 * Message body with skill mentions rendered as chips.
 *
 * The mention syntax is what the composer's palette inserts (`/id `), and the
 * runtime activates on the same token (`ark-skills::activation`). Rendering the
 * token as a chip makes the invocation visible without changing the text —
 * the copy button still copies the verbatim draft.
 *
 * `threadId` is the owning stream's id: mention chips need the thread's loaded
 * skill set, and this is the one legitimate fetch path for a history row (the
 * read is cached and shared with the composer's palette). `null` renders plain
 * text, so a detached row never becomes an API caller.
 */
function MentionBody({ text, threadId }: { text: string; threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const { skills } = useThreadSkills(threadId)
  if (!skills || skills.length === 0) return <>{text}</>
  return (
    <>
      {splitSkillMentions(text, skills).map((segment, index) =>
        segment.kind === 'mention' ? (
          <span
            key={`${index}-${segment.value}`}
            className="ark-mention"
            title={segment.skill.description ?? t('canvas.composer.skillsAria')}
          >
            <span className="ark-mention__glyph" aria-hidden="true">
              <svg viewBox="0 0 24 24" width="11" height="11" focusable="false">
                <path
                  d="M13 2.5 4.5 13.5H11L9.8 21.5 19.5 9.5H13l1.4-7Z"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.7"
                  strokeLinejoin="round"
                />
              </svg>
            </span>
            {segment.value}
          </span>
        ) : (
          <span key={`text-${index}`}>{segment.value}</span>
        )
      )}
    </>
  )
}

/** User message bubble. Text is the runtime's accumulated `text`, verbatim. */
export const UserBubble = memo(function UserBubble({ item, threadId = null }: { item: ThreadItem; threadId?: string | null }): JSX.Element {
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
      <div className="ark-msg__body">
        <MentionBody text={text} threadId={threadId} />
      </div>
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

/**
 * The turn's settled final reply, framed like smart-ark's answer card: a
 * check-marked "final reply" title row above a hairline rule, the prose below.
 *
 * Kept visually separate from the inline narration bubbles so the turn reads
 * as "process → answer". `AssistantBubble` stays for rows inside the process
 * card, where a card-in-card would double the chrome.
 */
export const FinalReplyCard = memo(function FinalReplyCard({ item }: { item: ThreadItem }): JSX.Element {
  const { t } = useI18n()
  const streaming = item.status === 'streaming' || item.status === 'pending'
  const failed = item.status === 'failed'
  const aborted = item.status === 'aborted'
  const text = item.text ?? ''

  return (
    <article className={`ark-reply${failed ? ' is-failed' : ''}`}>
      <header className="ark-reply__head">
        <span className="ark-reply__glyph" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="15" height="15" focusable="false">
            <circle cx="12" cy="12" r="9.25" fill="none" stroke="currentColor" strokeWidth="1.5" />
            <path d="m8.2 12.4 2.6 2.6 5-5.4" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </span>
        <span className="ark-reply__title">{t('canvas.reply.title')}</span>
        {aborted ? <span className="ark-chip ark-chip--muted">{t('canvas.aborted')}</span> : null}
        {failed ? <span className="ark-chip ark-chip--error">{t('canvas.failed')}</span> : null}
        {!streaming && text ? (
          <CopyButton
            text={text}
            label={t('canvas.copy.message')}
            copiedLabel={t('canvas.copy.copied')}
            className="ark-msg__copy ark-reply__copy"
          />
        ) : null}
      </header>
      <div className="ark-reply__body">
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
