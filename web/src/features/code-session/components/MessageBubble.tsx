import type { ThreadItem } from '../../../shared/contracts'
import { formatClock } from '../../../shared/format'

/** User message bubble. Text is the runtime's accumulated `text`, verbatim. */
export function UserBubble({ item }: { item: ThreadItem }): JSX.Element {
  return (
    <article className="ark-msg ark-msg--user">
      <header className="ark-msg__head">
        <span className="ark-msg__author">你</span>
        <time className="ark-msg__time">{formatClock(item.created_at)}</time>
      </header>
      <div className="ark-msg__body">{item.text ?? ''}</div>
    </article>
  )
}

/**
 * Assistant bubble. `streaming` only drives the caret: the text itself is never
 * re-parsed or re-assembled on the client.
 */
export function AssistantBubble({ item }: { item: ThreadItem }): JSX.Element {
  const streaming = item.status === 'streaming' || item.status === 'pending'
  const failed = item.status === 'failed'
  const aborted = item.status === 'aborted'

  return (
    <article className={`ark-msg ark-msg--assistant${failed ? ' is-failed' : ''}`}>
      <header className="ark-msg__head">
        <span className="ark-msg__author">ark</span>
        <time className="ark-msg__time">{formatClock(item.created_at)}</time>
        {aborted ? <span className="ark-chip ark-chip--muted">已中断</span> : null}
        {failed ? <span className="ark-chip ark-chip--error">失败</span> : null}
      </header>
      <div className="ark-msg__body ark-msg__body--prose">
        {item.text ?? ''}
        {streaming ? <span className="ark-caret" aria-label="正在生成" /> : null}
      </div>
      {item.detail ? <p className="ark-msg__detail">{item.detail}</p> : null}
    </article>
  )
}

/** Reasoning is collapsed by default and visually subordinate. */
export function ReasoningBlock({ item }: { item: ThreadItem }): JSX.Element {
  return (
    <details className="ark-reasoning">
      <summary className="ark-reasoning__summary">
        思考过程
        <span className="ark-reasoning__meta">{formatClock(item.created_at)}</span>
      </summary>
      <pre className="ark-reasoning__body">{item.text ?? ''}</pre>
    </details>
  )
}
