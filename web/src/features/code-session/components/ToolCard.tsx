import { memo, useState } from 'react'
import type { ThreadItem, ToolPayload } from '../../../shared/contracts'
import { formatToolArguments } from '../../../shared/format'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { toolTargetPath } from '../tool-file'

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
 * Tool family, resolved from the tool name: mainstream agent activity rows
 * give every entry a recognizable icon and a localized meaning label
 * ("终端命令", "读取文件") so a long turn scans by shape before the raw name.
 * Unknown names fall back to the wrench disc.
 */
type ToolFamily = 'read' | 'write' | 'exec' | 'search' | 'generic'

function toolFamily(tool: string): ToolFamily {
  const name = tool.toLowerCase()
  if (/(^|_)(read|view|open|list|show|fetch|get|download)(_|$)/.test(name)) return 'read'
  if (/(^|_)(write|edit|patch|apply|create|save|upload|move|rename)(_|$)/.test(name)) return 'write'
  if (/(^|_)(bash|exec|run|command|shell|terminal|cmd|powershell)(_|$)/.test(name)) return 'exec'
  if (/(^|_)(search|grep|find|glob|query|scan|web_search)(_|$)/.test(name)) return 'search'
  return 'generic'
}

const FAMILY_LABEL_KEY: Record<ToolFamily, MessageKey> = {
  read: 'canvas.tool.family.read',
  write: 'canvas.tool.family.write',
  exec: 'canvas.tool.family.exec',
  search: 'canvas.tool.family.search',
  generic: 'canvas.tool.family.generic'
}

/**
 * Lifecycle status node on the process card's timeline rail: green check for
 * a succeeded call, red cross for a failed one, spinning arc while it runs.
 * A settled call without an `ok` verdict reads as completed (check).
 */
function statusGlyphClass(item: ThreadItem, tool: ToolPayload): string {
  if (tool.ok === false) return 'ark-tool__status ark-tool__status--fail'
  if (item.status === 'streaming' || item.status === 'pending') return 'ark-tool__status ark-tool__status--live'
  return 'ark-tool__status ark-tool__status--ok'
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
 * File chip for write-family cards (requirements/tool-file-preview.md): shows
 * the basename of the file the tool wrote and opens it in the inspector's File
 * panel. A nested button inside the collapsible head must stop propagation, or
 * clicking the file would also toggle the card open.
 */
function FileChip({ path, name, dir }: { path: string; name: string; dir: string }): JSX.Element {
  const { t } = useI18n()
  const selectFile = useCodeSessionStore((s) => s.selectFile)
  return (
    <button
      type="button"
      className="ark-tool__filechip"
      title={t('canvas.tool.openFile', { path })}
      onClick={(event) => {
        event.stopPropagation()
        selectFile(path)
      }}
    >
      <span className="ark-tool__filechip-name">{name}</span>
      {dir ? <span className="ark-tool__filechip-dir">{dir}</span> : null}
    </button>
  )
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
  const target = toolTargetPath(tool)
  const family = toolFamily(tool.tool)

  return (
    <div className={`ark-tool${open ? ' is-open' : ''}`}>
      {/* The head is a row, not a single button: the file chip is an
          interactive control of its own, and nested <button>s are invalid DOM
          (validateDOMNesting) and break the outer click semantics. */}
      <div className="ark-tool__head">
        <button type="button" className="ark-tool__headtoggle" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          <span className={statusGlyphClass(item, tool)} aria-hidden="true" />
          <span className="ark-tool__chevron" aria-hidden="true">
            {open ? '▾' : '▸'}
          </span>
          <span className={`ark-tool__icon ark-tool__icon--${family}`} aria-hidden="true" />
          <span className="ark-tool__family">{t(FAMILY_LABEL_KEY[family])}</span>
          <span className="ark-tool__name">{tool.tool}</span>
        </button>
        {target ? <FileChip path={target.path} name={target.name} dir={target.dir} /> : null}
        {digest ? <span className="ark-tool__digest">{digest}</span> : null}
        <span className={`ark-chip ${statusClass(item, tool)}`}>{t(statusKey(item, tool))}</span>
        {typeof tool.duration_ms === 'number' ? (
          <span className="ark-tool__meta">{tool.duration_ms} ms</span>
        ) : null}
        <span className="ark-tool__callid">{tool.call_id}</span>
      </div>

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
