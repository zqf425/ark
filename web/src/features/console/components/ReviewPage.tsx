/**
 * Full-screen changes review (approved proposal A).
 *
 * Three columns: changed-file tree, a split/unified diff of the selected file,
 * and the review-notes rail. Data comes from `GET /v1/workspace/diff`; a review
 * turn is started through the existing `/review` endpoint when a thread is
 * active. Nothing here fabricates additions/deletions — counts are parsed from
 * the bounded patch the runtime returned.
 */
import { useEffect, useMemo, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { useI18n } from '../../../shared/i18n'
import { Icon } from '../../../shared/ui'
import type { WorkspaceDiff, WorkspaceDiffFile } from '../../../shared/contracts'
import { startReview } from '../../code-session/api/turns-api'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getWorkspaceDiff } from '../../settings/api/workspace-api'
import { useViewStore } from '../../workbench/state/view-slice'
import { diffLineCounts, parseUnifiedDiff, type DiffLine } from '../../workbench/panels/diff-lines'
import { buildSplitRows, tokenize } from '../review-diff'

type LoadStatus = 'loading' | 'ready' | 'error'

function Tokens({ text }: { text: string }): JSX.Element {
  return (
    <>
      {tokenize(text).map((token, index) => (
        token.cls
          ? <span key={index} className={token.cls}>{token.text}</span>
          : <span key={index}>{token.text}</span>
      ))}
    </>
  )
}

function statusClass(status: string): string {
  if (status.startsWith('A')) return 'A'
  if (status.startsWith('D')) return 'D'
  if (status.startsWith('?')) return 'U'
  return 'M'
}

function SplitCell({ line, side }: { line: DiffLine | null; side: 'left' | 'right' }): JSX.Element {
  if (!line) return <div className="console-diffln console-diffln--split" aria-hidden="true" />
  const number = side === 'left' ? line.oldNumber : line.newNumber
  const sign = line.type === 'add' ? '+' : line.type === 'remove' ? '−' : ' '
  return (
    <div className={`console-diffln console-diffln--split console-diffln--${line.type}`}>
      <span className="console-diffln__num">{number ?? ''}</span>
      <span className="console-diffln__sign">{sign}</span>
      <span className="console-diffln__text"><Tokens text={line.text} /></span>
    </div>
  )
}

function UnifiedRow({ line }: { line: DiffLine }): JSX.Element {
  return (
    <div className={`console-diffln console-diffln--${line.type}`}>
      <span className="console-diffln__num">{line.oldNumber ?? ''}</span>
      <span className="console-diffln__num">{line.newNumber ?? ''}</span>
      <span className="console-diffln__sign">
        {line.type === 'add' ? '+' : line.type === 'remove' ? '−' : ' '}
      </span>
      <span className="console-diffln__text">
        {line.type === 'meta' ? line.text : <Tokens text={line.text} />}
      </span>
    </div>
  )
}

export function ReviewPage(): JSX.Element {
  const { t } = useI18n()
  const setView = useViewStore((s) => s.setView)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)

  const [diff, setDiff] = useState<WorkspaceDiff | null>(null)
  const [status, setStatus] = useState<LoadStatus>('loading')
  const [error, setError] = useState<string | null>(null)
  const [tick, setTick] = useState(0)
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState<string | null>(null)
  const [mode, setMode] = useState<'split' | 'unified'>('split')
  const [reviewing, setReviewing] = useState(false)

  useEffect(() => {
    const controller = new AbortController()
    setStatus('loading')
    void getWorkspaceDiff(controller.signal)
      .then((next) => {
        if (controller.signal.aborted) return
        setDiff(next)
        setError(null)
        setStatus('ready')
      })
      .catch((reason) => {
        if (controller.signal.aborted) return
        setError(describeError(reason, translateNow))
        setStatus('error')
      })
    return () => controller.abort()
  }, [tick])

  const groups = useMemo<{ key: 'staged' | 'unstaged' | 'untracked'; files: WorkspaceDiffFile[] }[]>(() => {
    if (!diff) return []
    return [
      { key: 'staged', files: diff.staged },
      { key: 'unstaged', files: diff.unstaged },
      { key: 'untracked', files: diff.untracked }
    ].filter((group) => group.files.length > 0) as { key: 'staged' | 'unstaged' | 'untracked'; files: WorkspaceDiffFile[] }[]
  }, [diff])

  const allFiles = useMemo(() => groups.flatMap((group) => group.files), [groups])

  useEffect(() => {
    if (allFiles.length === 0) {
      if (selected !== null) setSelected(null)
      return
    }
    if (!selected || !allFiles.some((file) => file.path === selected)) {
      setSelected(allFiles[0]?.path ?? null)
    }
  }, [allFiles, selected])

  const selectedFile = allFiles.find((file) => file.path === selected) ?? null
  const lines = useMemo(
    () => (selectedFile?.patch ? parseUnifiedDiff(selectedFile.patch) : []),
    [selectedFile]
  )
  const counts = useMemo(() => diffLineCounts(lines), [lines])
  const splitRows = useMemo(() => buildSplitRows(lines), [lines])

  const filteredGroups = groups
    .map((group) => ({
      ...group,
      files: group.files.filter((file) => file.path.toLowerCase().includes(query.trim().toLowerCase()))
    }))
    .filter((group) => group.files.length > 0)

  const startReviewTurn = async (): Promise<void> => {
    if (!activeThreadId || reviewing) return
    setReviewing(true)
    try {
      await startReview(activeThreadId)
      setView('code')
    } catch (reason) {
      setError(describeError(reason, translateNow))
    } finally {
      setReviewing(false)
    }
  }

  return (
    <section className="console-review" aria-label={t('console.review.title')}>
      <aside className="console-review__tree" aria-label={t('console.review.files')}>
        <div className="console-panehead">
          <b>{t('console.review.files')}</b>
          <span className="console-panehead__grow" />
          <span className="console-panehead__count">{diff?.file_count ?? 0}</span>
        </div>
        <div className="console-review__filter">
          <Icon name="search" size={13} />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t('console.review.filter')}
            aria-label={t('console.review.filter')}
          />
        </div>
        <div className="console-review__files">
          {status === 'loading' ? (
            <p className="console-review__empty">{t('console.review.loading')}</p>
          ) : null}
          {status === 'error' ? (
            <p className="console-review__empty">{error}</p>
          ) : null}
          {status === 'ready' && allFiles.length === 0 ? (
            <p className="console-review__empty">{t('console.review.noChanges')}</p>
          ) : null}
          {filteredGroups.map((group) => (
            <div key={group.key}>
              <div className="console-review__grouplabel">
                {group.key === 'staged'
                  ? t('console.review.staged')
                  : group.key === 'unstaged'
                    ? t('console.review.unstaged')
                    : t('console.review.untracked')}
              </div>
              {group.files.map((file) => {
                const parts = file.path.split('/')
                const base = parts.pop() ?? file.path
                const dir = parts.join('/')
                return (
                  <button
                    key={`${group.key}:${file.path}`}
                    type="button"
                    className={`console-filerow${selected === file.path ? ' is-selected' : ''}`}
                    onClick={() => setSelected(file.path)}
                    aria-current={selected === file.path ? 'true' : undefined}
                    title={file.path}
                  >
                    <span className={`console-filerow__status console-filerow__status--${statusClass(file.status)}`}>
                      {file.status.slice(0, 1)}
                    </span>
                    <span className="console-filerow__name">
                      <b>{base}</b>
                      {dir ? <small>{dir}</small> : null}
                    </span>
                  </button>
                )
              })}
            </div>
          ))}
        </div>
      </aside>

      <div className="console-review__main">
        <div className="console-review__codehead">
          <Icon name="file" size={14} />
          <span className="console-filerow__name">
            <b>{selectedFile ? (selectedFile.path.split('/').pop() ?? selectedFile.path) : t('console.review.noSelection')}</b>
            {selectedFile ? <small>{selectedFile.path}</small> : null}
          </span>
          {selectedFile ? (
            <span className="console-panehead__count">
              <span className="console-diffln__sign" style={{ color: 'var(--console-add-ink)' }}>+{counts.additions}</span>{' '}
              <span className="console-diffln__sign" style={{ color: 'var(--console-del-ink)' }}>−{counts.deletions}</span>
            </span>
          ) : null}
          <span className="console-panehead__grow" />
          <div className="console-segmented" role="group" aria-label={t('console.review.title')}>
            <button type="button" className={mode === 'split' ? 'is-active' : ''} onClick={() => setMode('split')}>
              {t('console.review.split')}
            </button>
            <button type="button" className={mode === 'unified' ? 'is-active' : ''} onClick={() => setMode('unified')}>
              {t('console.review.unified')}
            </button>
          </div>
          <button
            type="button"
            className="console-iconbtn"
            onClick={() => setTick((value) => value + 1)}
            title={t('console.top.refresh')}
            aria-label={t('console.top.refresh')}
          >
            <Icon name="undo" size={14} />
          </button>
        </div>

        {selectedFile ? (
          selectedFile.binary ? (
            <p className="console-review__empty">{t('console.review.binary')}</p>
          ) : selectedFile.patch ? (
            <div className="console-diff">
              {mode === 'split' ? (
                <div className="console-diff__split">
                  <div className="console-diff__colhead">
                    <Icon name="file" size={12} /> {t('console.review.original')}
                  </div>
                  <div className="console-diff__colhead">
                    <Icon name="file" size={12} /> {t('console.review.modified')}
                  </div>
                  {splitRows.map((row, index) => (
                    row.kind === 'meta' ? (
                      <div
                        key={index}
                        className="console-diffln console-diffln--meta"
                        style={{ gridColumn: '1 / -1' }}
                      >
                        <span className="console-diffln__num" />
                        <span className="console-diffln__num" />
                        <span className="console-diffln__sign" />
                        <span className="console-diffln__text">{row.text}</span>
                      </div>
                    ) : (
                      <div key={index} style={{ display: 'contents' }}>
                        <SplitCell line={row.left} side="left" />
                        <SplitCell line={row.right} side="right" />
                      </div>
                    )
                  ))}
                </div>
              ) : (
                <div>
                  {lines.map((line, index) => <UnifiedRow key={index} line={line} />)}
                </div>
              )}
            </div>
          ) : (
            <p className="console-review__empty">{t('console.review.noPatch')}</p>
          )
        ) : (
          <p className="console-review__empty">{t('console.review.noSelection')}</p>
        )}
      </div>

      <aside className="console-review__notes" aria-label={t('console.review.notes')}>
        <div className="console-panehead">
          <b>{t('console.review.notes')}</b>
          <span className="console-panehead__grow" />
        </div>
        <div className="console-review__notes-body">
          <p>{t('console.review.notesEmpty')}</p>
          <p>{t('console.review.notesHint')}</p>
          {error && status !== 'error' ? <p style={{ color: 'var(--ark-error)' }}>{error}</p> : null}
        </div>
        <div className="console-review__foot">
          {activeThreadId ? (
            <button
              type="button"
              className="ark-btn ark-btn--primary ark-btn--sm"
              onClick={() => void startReviewTurn()}
              disabled={reviewing}
            >
              {t('canvas.header.review')}
            </button>
          ) : null}
          <button
            type="button"
            className="ark-btn ark-btn--sm"
            onClick={() => setView('code')}
          >
            {t('console.review.openWorkbench')}
          </button>
        </div>
      </aside>
    </section>
  )
}