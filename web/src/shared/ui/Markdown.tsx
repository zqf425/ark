import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { renderMarkdown } from './markdown-render'
import { copyText, COPIED_FEEDBACK_MS } from './copy'
import './markdown.css'

/** Minimum interval between markdown re-parses while text streams in. */
export const REPARSE_INTERVAL_MS = 120

/**
 * Coalesce rapid prop changes (streamed token deltas) into at most one value
 * per interval. Without this, an 80k-char message re-parses markdown on every
 * token and the canvas stalls; the final text always lands because the timer
 * fires with the latest value. A value equal to the one already shown never
 * schedules a timer, so static history messages mount without one.
 */
export function useThrottledValue<T>(value: T, intervalMs: number): T {
  const [deferred, setDeferred] = useState(value)
  const lastRunAt = useRef(0)
  const shown = useRef(value)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    if (Object.is(value, shown.current)) return
    const run = (): void => {
      lastRunAt.current = Date.now()
      timer.current = null
      shown.current = value
      setDeferred(value)
    }
    const wait = Math.max(0, lastRunAt.current + intervalMs - Date.now())
    timer.current = setTimeout(run, wait)
    return () => {
      if (timer.current) clearTimeout(timer.current)
    }
  }, [value, intervalMs])

  return deferred
}

/**
 * Rendered model prose. The HTML is produced by `markdown-render` (marked +
 * DOMPurify), so `dangerouslySetInnerHTML` here carries sanitized markup only.
 * Streaming updates are throttled: a mid-turn row re-parses at most
 * `REPARSE_INTERVAL_MS` times per second instead of on every token.
 *
 * Code blocks carry their own copy button (emitted by the renderer). One
 * delegated click handler serves every block of this message — streaming
 * replaces the DOM wholesale, so per-block listeners would leak or churn.
 */
export const Markdown = memo(function Markdown({
  text,
  className,
  onFileChip,
  writablePaths
}: {
  text: string
  className?: string
  /** Fired when a prose file chip (`data-md-file`) is clicked. Absent = chips
   * never render, so consumers opt in and `shared/ui` stays feature-agnostic. */
  onFileChip?: (path: string) => void
  /** This thread's written-path whitelist (requirements/prose-path-links.md);
   * passed through to `renderMarkdown`'s promotion pass. Empty = no chips. */
  writablePaths?: readonly string[]
}): JSX.Element {
  const throttled = useThrottledValue(text, REPARSE_INTERVAL_MS)
  const writable = writablePaths ?? []
  const writableKey = writable.join('\u0000')
  const html = useMemo(
    () => renderMarkdown(throttled, writable),
    // join() gives useMemo a stable primitive key without sorting the array.
    [throttled, writableKey]
  )
  const hostRef = useRef<HTMLDivElement | null>(null)
  // Streamed rows re-render wholesale; a per-block "copied" state would be
  // lost on the next parse, so the flip lives on the shared handler instead.
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  // The handler identity rides a ref so the memoized row never re-renders
  // because the parent passed a fresh lambda each render.
  const fileChipRef = useRef(onFileChip)
  fileChipRef.current = onFileChip

  useEffect(
    () => () => {
      if (copiedTimer.current) clearTimeout(copiedTimer.current)
    },
    []
  )

  const onClick = (event: React.MouseEvent<HTMLDivElement>): void => {
    const fileChip = (event.target as HTMLElement).closest('[data-md-file]')
    if (fileChip) {
      event.stopPropagation()
      fileChipRef.current?.(fileChip.getAttribute('data-md-file') ?? '')
      return
    }
    const target = (event.target as HTMLElement).closest('[data-md-copy]')
    if (!target) return
    const source = target.getAttribute('data-copy-source') ?? ''
    const button = target as HTMLButtonElement
    void copyText(source).then((ok) => {
      if (!ok) return
      button.classList.add('is-copied')
      if (copiedTimer.current) clearTimeout(copiedTimer.current)
      copiedTimer.current = setTimeout(() => button.classList.remove('is-copied'), COPIED_FEEDBACK_MS)
    })
  }

  return (
    <div
      ref={hostRef}
      className={className ? `ark-md ${className}` : 'ark-md'}
      onClick={onClick}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
})
