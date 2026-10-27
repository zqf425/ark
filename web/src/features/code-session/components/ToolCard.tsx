import { useState } from 'react'
import type { ThreadItem, ToolPayload } from '../../../shared/contracts'
import { formatToolArguments } from '../../../shared/format'

function statusLabel(item: ThreadItem, tool: ToolPayload): string {
  if (tool.ok === true) return '成功'
  if (tool.ok === false) return '失败'
  return item.status === 'streaming' || item.status === 'pending' ? '运行中' : '已完成'
}

function statusClass(item: ThreadItem, tool: ToolPayload): string {
  if (tool.ok === false) return 'ark-chip--error'
  if (item.status === 'streaming' || item.status === 'pending') return 'ark-chip--live'
  return 'ark-chip--ok'
}

/**
 * Foldable tool card. The `tool` payload is the runtime's structured facet, so
 * nothing here infers the tool name from text.
 */
export function ToolCard({ item }: { item: ThreadItem }): JSX.Element {
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
  const hasBody = Boolean(args || tool.output || item.detail)

  return (
    <div className={`ark-tool${open ? ' is-open' : ''}`}>
      <button type="button" className="ark-tool__head" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        <span className="ark-tool__chevron" aria-hidden="true">
          {open ? '▾' : '▸'}
        </span>
        <span className="ark-tool__name">{tool.tool}</span>
        <span className={`ark-chip ${statusClass(item, tool)}`}>{statusLabel(item, tool)}</span>
        {typeof tool.duration_ms === 'number' ? (
          <span className="ark-tool__meta">{tool.duration_ms} ms</span>
        ) : null}
        <span className="ark-tool__callid">{tool.call_id}</span>
      </button>

      {open && hasBody ? (
        <div className="ark-tool__body">
          {args ? (
            <section className="ark-tool__section">
              <h4 className="ark-tool__label">参数</h4>
              <pre className="ark-tool__output">{args}</pre>
            </section>
          ) : null}
          {tool.output ? (
            <section className="ark-tool__section">
              <h4 className="ark-tool__label">输出</h4>
              <pre className="ark-tool__output">{tool.output}</pre>
            </section>
          ) : null}
          {item.detail ? <p className="ark-tool__note">{item.detail}</p> : null}
        </div>
      ) : null}
    </div>
  )
}
