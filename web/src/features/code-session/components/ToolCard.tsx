import { memo, useState } from 'react'
import type { ThreadItem, ToolPayload } from '../../../shared/contracts'
import { formatToolArguments } from '../../../shared/format'
import { useI18n, type MessageKey } from '../../../shared/i18n'

function statusKey(item: ThreadItem, tool: ToolPayload): MessageKey {
  if (tool.ok === true) return 'canvas.tool.succeeded'
  if (tool.ok === false) return 'canvas.tool.failed'
  return item.status === 'streaming' || item.status === 'pending' ? 'canvas.tool.running' : 'canvas.tool.completed'
}

function statusClass(item: ThreadItem, tool: ToolPayload): string {
  if (tool.ok === false) return 'ark-chip--error'
  if (item.status === 'streaming' || item.status === 'pending') return 'ark-chip--live'
  return 'ark-chip--ok'
}

/**
 * Tool-family glyph class, resolved from the tool name: mainstream agent
 * activity rows give every entry a recognizable icon so a long turn scans by
 * shape. Unknown names fall back to the wrench disc.
 */
function toolIconClass(tool: string): string {
  const name = tool.toLowerCase()
  if (/(^|_)(read|view|open|list|show|fetch|get|download)(_|$)/.test(name)) return 'ark-tool__icon--read'
  if (/(^|_)(write|edit|patch|apply|create|save|upload|move|rename)(_|$)/.test(name)) return 'ark-tool__icon--write'
  if (/(^|_)(bash|exec|run|command|shell|terminal|cmd|powershell)(_|$)/.test(name)) return 'ark-tool__icon--exec'
  if (/(^|_)(search|grep|find|glob|query|scan|web_search)(_|$)/.test(name)) return 'ark-tool__icon--search'
  return 'ark-tool__icon--generic'
}

/**
 * One-line argument digest for the collapsed header (e.g. `path=web/src`, or
 * `cmd=npm test`). The full JSON stays behind the fold; the digest only reads
 * already-formatted text, so no second serializer exists. Returns '' when the
 * arguments are not a plain object.
 */
function argsDigest(formatted: string): string {
  if (!formatted.startsWith('{')) return ''
  try {
    const parsed = JSON.parse(formatted) as Record<string, unknown>
    const first = Object.entries(parsed)[0]
    if (!first) return ''
    const [key, value] = first
    const text = typeof value === 'string' ? value : JSON.stringify(value)
    return `${key}=${text}`
  } catch {
    return ''
  }
}

/**
 * Foldable tool card. The `tool` payload is the runtime's structured facet, so
 * nothing here infers the tool name from text.
 *
 * Memoized on its `item` object with `MessageBubble`: a tool card is by far the
 * most common row in a long turn, and it re-renders only when its own item does.
 */
export const ToolCard = memo(function ToolCard({ item }: { item: ThreadItem }): JSX.Element {
  const { t } = useI18n()
  const tool = item.tool
  const [open, setOpen] = useState(false)
  if (!tool) {
    // Defensive: a tool item without a payload still shows up, never disappears.
    return (
      <div className="ark-tool ark-tool--bare">
        <span className="ark-tool__name">tool</span>
        <pre className="ark-tool__output">{item.text ?? item.detail ?? ''}</pre>
      </div>
    )
  }

  const args = formatToolArguments(tool.arguments)
  const digest = argsDigest(args)
  const hasBody = Boolean(args || tool.output || item.detail)

  return (
    <div className={`ark-tool${open ? ' is-open' : ''}`}>
      <button type="button" className="ark-tool__head" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        <span className="ark-tool__chevron" aria-hidden="true">
          {open ? '▾' : '▸'}
        </span>
        <span className={`ark-tool__icon ${toolIconClass(tool.tool)}`} aria-hidden="true" />
        <span className="ark-tool__name">{tool.tool}</span>
        {digest ? <span className="ark-tool__digest">{digest}</span> : null}
        <span className={`ark-chip ${statusClass(item, tool)}`}>{t(statusKey(item, tool))}</span>
        {typeof tool.duration_ms === 'number' ? (
          <span className="ark-tool__meta">{tool.duration_ms} ms</span>
        ) : null}
        <span className="ark-tool__callid">{tool.call_id}</span>
      </button>

      {open && hasBody ? (
        <div className="ark-tool__body">
          {args ? (
            <section className="ark-tool__section">
              <h4 className="ark-tool__label">{t('canvas.tool.arguments')}</h4>
              <pre className="ark-tool__output">{args}</pre>
            </section>
          ) : null}
          {tool.output ? (
            <section className="ark-tool__section">
              <h4 className="ark-tool__label">{t('canvas.tool.output')}</h4>
              <pre className="ark-tool__output">{tool.output}</pre>
            </section>
          ) : null}
          {item.detail ? <p className="ark-tool__note">{item.detail}</p> : null}
        </div>
      ) : null}
    </div>
  )
})
