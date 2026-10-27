import { useState } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { formatClock } from '../../../shared/format'

/**
 * Inline error row. `code` comes straight from the runtime error taxonomy
 * (design §6.2); the message is shown verbatim so operators can search logs.
 */
export function ErrorRow({ item }: { item: ThreadItem }): JSX.Element {
  return (
    <div className="ark-errorrow" role="alert">
      <span className="ark-errorrow__mark">!</span>
      <div className="ark-errorrow__body">
        <p className="ark-errorrow__message">{item.text ?? item.detail ?? '未知错误'}</p>
        <span className="ark-errorrow__meta">{formatClock(item.created_at)}</span>
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

/**
 * Tool result without a preceding call card: the runtime sends standalone
 * results when a tool is retried, so it gets its own collapsible row.
 */
export function ToolResultRow({ item }: { item: ThreadItem }): JSX.Element {
  const [open, setOpen] = useState(false)
  const output = item.tool?.output ?? item.text ?? ''
  const ok = item.tool?.ok

  return (
    <div className="ark-toolresult">
      <button type="button" className="ark-toolresult__head" onClick={() => setOpen((v) => !v)}>
        <span className={`ark-chip ${ok === false ? 'ark-chip--error' : 'ark-chip--ok'}`}>
          {ok === false ? '失败' : '完成'}
        </span>
        <span className="ark-toolresult__name">{item.tool?.tool ?? 'tool result'}</span>
        <span className="ark-toolresult__meta">{open ? '收起' : '展开'}</span>
      </button>
      {open ? <pre className="ark-tool__output">{output}</pre> : null}
    </div>
  )
}
