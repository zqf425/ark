import { useEffect, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { TerminalEntriesResponse } from '../../../shared/contracts'
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
  const lastSeq = useCodeSessionStore((state) => state.lastSeq)
  const [data, setData] = useState<TerminalEntriesResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!threadId) return
    const controller = new AbortController()
    void getTerminalEntries(threadId, controller.signal).then((next) => {
      setData(next)
      setError(null)
    }).catch((reason) => {
      if (!controller.signal.aborted) setError(describe(reason))
    })
    return () => controller.abort()
  }, [threadId, lastSeq])

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
