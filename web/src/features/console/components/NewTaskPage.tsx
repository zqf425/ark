/**
 * New-task page: pick an intent, describe the goal, start.
 *
 * Code and Plan both create a thread (Plan stops there so the user can write
 * the planning prompt in the composer); Review switches to the changes review
 * page instead of creating a thread. The button is disabled while the request
 * is in flight, and failures surface through the shared toast.
 */
import { useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { useI18n } from '../../../shared/i18n'
import { Icon, useToast } from '../../../shared/ui'
import { createThread } from '../../code-session/api/threads-api'
import { startTurn } from '../../code-session/api/turns-api'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useViewStore } from '../../workbench/state/view-slice'
import type { ConsoleMeta } from '../hooks/use-console-meta'

type Mode = 'code' | 'plan' | 'review'

const MODES: { id: Mode; icon: string; labelKey: 'console.new.modeCode' | 'console.new.modePlan' | 'console.new.modeReview'; hintKey: 'console.new.modeCodeHint' | 'console.new.modePlanHint' | 'console.new.modeReviewHint' }[] = [
  { id: 'code', icon: 'code', labelKey: 'console.new.modeCode', hintKey: 'console.new.modeCodeHint' },
  { id: 'plan', icon: 'list', labelKey: 'console.new.modePlan', hintKey: 'console.new.modePlanHint' },
  { id: 'review', icon: 'diff', labelKey: 'console.new.modeReview', hintKey: 'console.new.modeReviewHint' }
]

const EXAMPLE_KEYS = [
  'console.new.example1',
  'console.new.example2',
  'console.new.example3',
  'console.new.example4'
] as const

export function NewTaskPage({ meta }: { meta: ConsoleMeta }): JSX.Element {
  const { t } = useI18n()
  const push = useToast().push
  const setView = useViewStore((s) => s.setView)
  const setActiveThread = useCodeSessionStore((s) => s.setActiveThread)
  const upsertThread = useCodeSessionStore((s) => s.upsertThread)

  const [mode, setMode] = useState<Mode>('code')
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)

  const start = async (): Promise<void> => {
    if (busy) return
    if (mode === 'review') {
      setView('review')
      return
    }
    const text = draft.trim()
    setBusy(true)
    try {
      const created = await createThread({})
      upsertThread(created)
      setActiveThread(created.thread_id)
      if (mode === 'code' && text) {
        await startTurn(created.thread_id, { text, attachment_ids: [] })
      }
      setView('code')
    } catch (error) {
      push('error', t('console.new.failed'), describeError(error, translateNow))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="console-new" aria-label={t('console.new.title')}>
      <div className="console-new__wrap">
        <div className="console-new__kicker">
          <span className="console-mark console-mark--sm" aria-hidden="true">A</span>
          {t('console.new.kicker')}
        </div>
        <h2>{t('console.new.title')}</h2>
        <p className="console-new__sub">{t('console.new.subtitle')}</p>

        <div className="console-new__modes" role="radiogroup" aria-label={t('console.new.title')}>
          {MODES.map((entry) => (
            <button
              key={entry.id}
              type="button"
              role="radio"
              aria-checked={mode === entry.id}
              className={`console-mode${mode === entry.id ? ' is-selected' : ''}`}
              onClick={() => setMode(entry.id)}
            >
              <span className="console-mode__top">
                <span className="console-mode__icon"><Icon name={entry.icon} size={14} /></span>
                {t(entry.labelKey)}
              </span>
              <p>{t(entry.hintKey)}</p>
            </button>
          ))}
        </div>

        <div className="console-new__box">
          <textarea
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            placeholder={t('console.new.placeholder')}
            aria-label={t('console.new.placeholder')}
            onKeyDown={(event) => {
              if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
                event.preventDefault()
                void start()
              }
            }}
          />
          <div className="console-new__foot">
            <span className="console-chip" title={meta.workspacePath ?? ''}>
              <span className="console-chip__icon"><Icon name="folder" size={13} /></span>
              <span className="console-chip__path">{meta.workspacePath ?? t('console.new.workspace')}</span>
            </span>
            {meta.headSha ? (
              <span className="console-chip">
                <span className="console-chip__icon"><Icon name="git" size={13} /></span>
                <span className="console-chip__path">{meta.headSha.slice(0, 7)}</span>
              </span>
            ) : null}
            <span className="console-chip">
              <span className="console-chip__icon"><Icon name="spark" size={13} /></span>
              <span className="console-chip__path">{meta.modelLabel ?? t('console.top.model')}</span>
            </span>
            <span className="console-chip">
              <span className="console-chip__icon"><Icon name="shield" size={13} /></span>
              <span className="console-chip__path">{t('console.new.standard')}</span>
            </span>
            <span className="console-new__foot-grow" />
            <button
              type="button"
              className="ark-btn ark-btn--primary ark-btn--sm"
              onClick={() => void start()}
              disabled={busy}
            >
              {busy ? t('console.new.starting') : t('console.new.start')}
              <kbd className="console-kbd">↵</kbd>
            </button>
          </div>
        </div>

        <div className="console-new__examples">
          {EXAMPLE_KEYS.map((key) => (
            <button key={key} type="button" onClick={() => setDraft(t(key))}>
              {t(key)}
            </button>
          ))}
        </div>

        <div className="console-new__recent">
          <Icon name="clock" size={12} />
          {t('console.new.planHint')}
        </div>
      </div>
    </section>
  )
}