/**
 * Clipboard copy helpers, the single copy path for every surface (bubbles,
 * code blocks, composer). `copyText` hides the secure-context fallback dance
 * (async Clipboard API first, `execCommand` for older webviews), and
 * `CopyButton` owns the pressed→copied feedback so call sites never duplicate
 * the transient "copied" state machine.
 */
import { useEffect, useRef, useState } from 'react'
import './copy.css'

/** How long the "copied" check mark stays visible before reverting. */
export const COPIED_FEEDBACK_MS = 1_600

/** Legacy in-page copy for non-secure contexts (Tauri http://localhost). */
function copyViaExecCommand(text: string): boolean {
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.appendChild(area)
  area.select()
  let ok = false
  try {
    ok = document.execCommand('copy')
  } catch {
    ok = false
  }
  area.remove()
  return ok
}

/**
 * Copy `text` and report success. Never throws: a denied clipboard is a failed
 * copy the button surfaces as "not copied", not an error row.
 */
export async function copyText(text: string): Promise<boolean> {
  if (!text) return false
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // Fall through to the legacy path; some webviews reject the promise for
    // non-focused documents but still allow execCommand.
  }
  return copyViaExecCommand(text)
}

/** Copy glyphs (two sheets) and its pressed state (a check), stroke-drawn. */
function CopyGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
      <rect x="9" y="9" width="11" height="11" rx="2" fill="none" stroke="currentColor" strokeWidth="1.7" />
      <path d="M5 15H4.5A1.5 1.5 0 0 1 3 13.5v-9A1.5 1.5 0 0 1 4.5 3h9A1.5 1.5 0 0 1 15 4.5V5" fill="none" stroke="currentColor" strokeWidth="1.7" />
    </svg>
  )
}

function CheckGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
      <path d="m5 12.5 4.5 4.5L19 7" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

/**
 * Quiet icon button that copies `text` and flips to a check for a moment.
 * `label` / `copiedLabel` come from i18n at the call site; keep this component
 * i18n-free so non-i18n hosts (pure utility usages) stay possible.
 */
export function CopyButton({
  text,
  label,
  copiedLabel,
  className,
  onCopied
}: {
  text: string
  label: string
  copiedLabel: string
  className?: string
  onCopied?: (copied: boolean) => void
}): JSX.Element {
  const [copied, setCopied] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current)
    },
    []
  )

  const copy = async (): Promise<void> => {
    const ok = await copyText(text)
    onCopied?.(ok)
    if (!ok) return
    setCopied(true)
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS)
  }

  const stateClass = copied ? ' is-copied' : ''
  return (
    <button
      type="button"
      className={`ark-copy${stateClass}${className ? ` ${className}` : ''}`}
      aria-label={copied ? copiedLabel : label}
      title={copied ? copiedLabel : label}
      onClick={(event) => {
        // Keep ancestors (card headers, tool summaries) from reacting.
        event.stopPropagation()
        void copy()
      }}
      onMouseDown={(event) => event.stopPropagation()}
    >
      {copied ? <CheckGlyph /> : <CopyGlyph />}
    </button>
  )
}
