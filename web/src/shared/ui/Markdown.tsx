import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { renderMarkdown } from './markdown-render'
import './markdown.css'

/** Minimum interval between markdown re-parses while text streams in. */
const REPARSE_INTERVAL_MS = 120

/**
 * Coalesce rapid prop changes (streamed token deltas) into at most one value
 * per interval. Without this, an 80k-char message re-parses markdown on every
 * token and the canvas stalls; the final text always lands because the timer
 * fires with the latest value.
 */
function useThrottledValue<T>(value: T, intervalMs: number): T {
  const [deferred, setDeferred] = useState(value)
  const lastRunAt = useRef(0)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    const run = (): void => {
      lastRunAt.current = Date.now()
      timer.current = null
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
 */
export const Markdown = memo(function Markdown({ text, className }: { text: string; className?: string }): JSX.Element {
  const throttled = useThrottledValue(text, REPARSE_INTERVAL_MS)
  const html = useMemo(() => renderMarkdown(throttled), [throttled])
  return <div className={className ? `ark-md ${className}` : 'ark-md'} dangerouslySetInnerHTML={{ __html: html }} />
})
