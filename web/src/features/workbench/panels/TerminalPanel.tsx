import { useEffect, useRef, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { ThreadId, TerminalEntriesResponse } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getTerminalEntries } from '../api/workbench-api'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export function TerminalPanel(): JSX.Element {
  const { t } = useI18n()
  const threadId = useCodeSessionStore((state) => state.activeThreadId)
  const workbenchRevision = useCodeSessionStore((state) => state.workbenchRevision)
  const [data, setData] = useState<TerminalEntriesResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  /**
   * Which thread the entries and the newest terminal request belong to.
   * Bookkeeping, never render input: the panel stays mounted across thread
   * switches while its entries are per-thread, so "these entries are A's" has to
   * be an explicit fact.
   */
  const [rowsFor, setRowsFor] = useState<ThreadId | null>(threadId)
  const latestLoad = useRef<{ threadId: ThreadId | null; ticket: number }>({ threadId, ticket: 0 })

  // Switching threads drops the previous thread's entries in the same commit, and
  // the thread change in `latestLoad` invalidates every request still in flight
  // for it.
  if (rowsFor !== threadId) {
    setRowsFor(threadId)
    setData(null)
    setError(null)
    latestLoad.current = { threadId, ticket: latestLoad.current.ticket + 1 }
  }

  /**
   * One terminal request. A response paints only while it is still the newest
   * request for the thread on screen: a reload can otherwise resolve out of
   * order, and the previous thread's entries must never reappear over the
   * current one.
   */
  const load = (forThread: ThreadId, signal: AbortSignal): void => {
    const ticket = latestLoad.current.ticket + 1
    latestLoad.current = { threadId: forThread, ticket }
    const isCurrent = (): boolean => !signal.aborted && latestLoad.current.threadId === forThread && latestLoad.current.ticket === ticket
    void getTerminalEntries(forThread, signal).then((next) => {
      if (!isCurrent()) return
      setData(next)
      setError(null)
    }).catch((reason) => {
      if (!isCurrent()) return
      setData(null)
      setError(describe(reason))
    })
  }

  useEffect(() => {
    // A cleared selection is handled by the render-phase sync above, so there is
    // nothing to load here.
    if (!threadId) return
    const controller = new AbortController()
    load(threadId, controller.signal)
    return () => controller.abort()
  }, [threadId, workbenchRevision])

  if (!threadId) {
    return <EmptyState title={t('workbench.noThreadTitle')} hint={t('workbench.noThreadTerminalHint')} />
  }
  if (error) return <p className="ark-panel__error">{error}</p>
  if (!data) return <p className="ark-panel__hint">{t('workbench.terminal.loading')}</p>
  if (data.entries.length === 0) {
    return <EmptyState title={t('workbench.terminal.emptyTitle')} hint={t('workbench.terminal.emptyHint')} />
  }
  return (
    <section className="ark-panel">
      <header className="ark-panel__head">
        <h2>{t('workbench.terminal.title')}</h2>
        <span>{t('workbench.terminal.count', { count: data.entries.length })}</span>
      </header>
      <div className="ark-terminal__list">
        {data.entries.map((entry) => (
          <details className="ark-terminal" key={`${entry.call_id}:${entry.seq}`}>
            <summary>
              <span className={entry.ok ? 'ark-chip ark-chip--ok' : 'ark-chip ark-chip--error'}>
                {entry.ok ? t('workbench.terminal.ok') : t('workbench.terminal.failed')}
              </span>
              <code>{entry.command}</code>
              <span>{entry.duration_ms} ms</span>
            </summary>
            <pre className="ark-panel__code">{entry.output || t('workbench.terminal.noOutput')}</pre>
          </details>
        ))}
      </div>
    </section>
  )
}
