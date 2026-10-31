import { useState } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { formatClock } from '../../../shared/format'
import { useI18n } from '../../../shared/i18n'
import { childCompletionStatusKey } from '../child-completion-status'
import type { ChildCompletionNotice } from '../state/types'

/**
 * Inline error row. `code` comes straight from the runtime error taxonomy
 * (design §6.2); the message is shown verbatim so operators can search logs.
 * The fallback is client copy and therefore localized.
 */
export function ErrorRow({ item }: { item: ThreadItem }): JSX.Element {
  const { t, locale } = useI18n()
  return (
    <div className="ark-errorrow" role="alert">
      <span className="ark-errorrow__mark">!</span>
      <div className="ark-errorrow__body">
        <p className="ark-errorrow__message">{item.text ?? item.detail ?? t('common.unknownError')}</p>
        <span className="ark-errorrow__meta">{formatClock(item.created_at, locale)}</span>
      </div>
    </div>
  )
}

/** Neutral notice row (compaction, abort reason, notices from the loop). */
export function NoticeRow({ item }: { item: ThreadItem }): JSX.Element {
  return (
    <div className="ark-notice">
      <span className="ark-notice__text">{item.text ?? item.detail ?? ''}</span>
    </div>
  )
}

/** Durable parent-thread summary for one finished background child. */
export function ChildCompletionCard({ notice }: { notice: ChildCompletionNotice }): JSX.Element {
  const { t, locale } = useI18n()
  const failed = notice.status !== 'completed'
  const detail = notice.summary ?? notice.error ?? t('canvas.child.noSummary')

  return (
    <div className={`ark-childcompletion ${failed ? 'is-failed' : ''}`} role="status">
      <div className="ark-childcompletion__head">
        <span className={`ark-chip ${failed ? 'ark-chip--error' : 'ark-chip--ok'}`}>
          {t(childCompletionStatusKey(notice.status))}
        </span>
        <strong>{notice.label}</strong>
        <span className="ark-childcompletion__time">{formatClock(notice.completed_at, locale)}</span>
      </div>
      <p className="ark-childcompletion__summary">{detail}</p>
    </div>
  )
}

/**
 * Tool result without a preceding call card: the runtime sends standalone
 * results when a tool is retried, so it gets its own collapsible row.
 */
export function ToolResultRow({ item }: { item: ThreadItem }): JSX.Element {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const output = item.tool?.output ?? item.text ?? ''
  const ok = item.tool?.ok

  return (
    <div className="ark-toolresult">
      <button type="button" className="ark-toolresult__head" onClick={() => setOpen((v) => !v)}>
        <span className={`ark-chip ${ok === false ? 'ark-chip--error' : 'ark-chip--ok'}`}>
          {ok === false ? t('common.failed') : t('canvas.tool.completed')}
        </span>
        <span className="ark-toolresult__name">{item.tool?.tool ?? t('canvas.toolResult.name')}</span>
        <span className="ark-toolresult__meta">{open ? t('common.collapse') : t('common.expand')}</span>
      </button>
      {open ? <pre className="ark-tool__output">{output}</pre> : null}
    </div>
  )
}
