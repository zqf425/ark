/**
 * Memory section: list, create, edit, delete plus storage diagnostics.
 *
 * The runtime injects every stored entry into each turn (up to its own
 * budget), and there is deliberately no enable switch — deleting an entry is
 * how it stops being injected (docs/requirements/memory-store.md).
 */
import { useState } from 'react'
import type { MemoryEntry } from '../../../shared/contracts'
import { Button, EmptyState, useToast, type ToastTone } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useMemory, type MemoryOutcome } from '../hooks/use-memory'
import { KeyValue, LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'
import { memoryTextError, parseMemoryTags } from './memory-form'

function outcomeToast(
  push: (tone: ToastTone, message: string, detail?: string) => void,
  outcome: MemoryOutcome,
  successTitle: string,
  failureTitle: string
): void {
  if (outcome.ok) push('success', successTitle)
  else push('error', failureTitle, outcome.error)
}

/** One stored memory: text, tags, source, and the edit/delete affordances. */
function MemoryCard(props: {
  entry: MemoryEntry
  onDelete: (id: string) => void
  onUpdate: (id: string, text: string, tags: string[], source: string) => void
}): JSX.Element {
  const { t, locale } = useI18n()
  const { entry, onDelete, onUpdate } = props
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')
  const [draftTags, setDraftTags] = useState('')
  const [draftSource, setDraftSource] = useState('')
  const [draftError, setDraftError] = useState<string | null>(null)

  const beginEdit = (): void => {
    setDraft(entry.text)
    setDraftTags(entry.tags.join(', '))
    setDraftSource(entry.source ?? '')
    setDraftError(null)
    setEditing(true)
  }

  const save = (): void => {
    const problem = memoryTextError(draft)
    if (problem) {
      setDraftError(t(problem))
      return
    }
    onUpdate(entry.id, draft, parseMemoryTags(draftTags), draftSource.trim())
    setEditing(false)
  }

  return (
    <div className="ark-card">
      {editing ? (
        <div className="ark-settings__stack">
          <textarea
            value={draft}
            rows={4}
            spellCheck={false}
            aria-label={t('settings.memory.textLabel')}
            onChange={(event) => setDraft(event.target.value)}
          />
          {draftError ? <p className="ark-settings__hint ark-settings__hint--error">{draftError}</p> : null}
          <label className="ark-settings__hint">
            {t('settings.memory.tagsLabel')}
            <input
              value={draftTags}
              spellCheck={false}
              autoComplete="off"
              placeholder={t('settings.memory.tagsPlaceholder')}
              onChange={(event) => setDraftTags(event.target.value)}
            />
          </label>
          <label className="ark-settings__hint">
            {t('settings.memory.sourceLabel')}
            <input
              value={draftSource}
              spellCheck={false}
              autoComplete="off"
              onChange={(event) => setDraftSource(event.target.value)}
            />
          </label>
          <div className="ark-card__actions">
            <Button size="sm" onClick={save}>{t('common.save')}</Button>
            <Button size="sm" variant="ghost" onClick={() => setEditing(false)}>{t('common.cancel')}</Button>
          </div>
        </div>
      ) : (
        <>
          <p className="ark-memory__text">{entry.text}</p>
          <div className="ark-card__meta">
            {entry.tags.map((tag) => (
              <span key={tag} className="ark-chip">{tag}</span>
            ))}
            {entry.source ? <span className="ark-chip ark-chip--muted">{entry.source}</span> : null}
            <span>{new Date(entry.updated_at).toLocaleString(locale)}</span>
          </div>
          <div className="ark-card__actions">
            <Button size="sm" variant="ghost" onClick={beginEdit}>{t('common.edit')}</Button>
            <Button size="sm" variant="ghost" onClick={() => onDelete(entry.id)}>{t('common.delete')}</Button>
          </div>
        </>
      )}
    </div>
  )
}

export function MemorySection(): JSX.Element {
  const { t } = useI18n()
  const api = useMemory()
  const { push: pushToast } = useToast()
  const [text, setText] = useState('')
  const [tags, setTags] = useState('')
  const [source, setSource] = useState('')
  const [formError, setFormError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const create = async (): Promise<void> => {
    const problem = memoryTextError(text)
    if (problem) {
      setFormError(t(problem))
      return
    }
    setSaving(true)
    const outcome = await api.create({ text, tags: parseMemoryTags(tags), source: source.trim() || undefined })
    setSaving(false)
    outcomeToast(pushToast, outcome, t('settings.memory.created'), t('settings.memory.createFailed'))
    if (outcome.ok) {
      setText('')
      setTags('')
      setSource('')
      setFormError(null)
    }
  }

  const remove = async (memoryId: string): Promise<void> => {
    const outcome = await api.remove(memoryId)
    outcomeToast(pushToast, outcome, t('settings.memory.deleted'), t('settings.memory.deleteFailed'))
  }

  const update = async (memoryId: string, newText: string, newTags: string[], newSource: string): Promise<void> => {
    const outcome = await api.update(memoryId, {
      text: newText,
      tags: newTags,
      source: newSource.length > 0 ? newSource : undefined
    })
    outcomeToast(pushToast, outcome, t('settings.memory.updated'), t('settings.memory.updateFailed'))
  }

  const diag = api.diagnostics
  return (
    <SettingsSection
      title={t('settings.memory.title')}
      desc={t('settings.memory.desc')}
      actions={
        <Button size="sm" disabled={api.status === 'loading'} onClick={() => void api.refresh()}>
          {api.status === 'loading' ? t('common.refreshing') : t('common.refresh')}
        </Button>
      }
    >
      {diag ? (
        <KeyValue
          rows={[
            { key: t('settings.memory.storagePath'), value: <code className="ark-settings__code">{diag.storage_path}</code> },
            { key: t('settings.memory.entryCount'), value: diag.entry_count },
            {
              key: t('settings.memory.lastWritten'),
              value: diag.last_written_at ? new Date(diag.last_written_at).toLocaleString() : t('settings.memory.never')
            }
          ]}
        />
      ) : null}

      <div className="ark-settings__stack">
        <label className="ark-settings__hint">
          {t('settings.memory.textLabel')}
          <textarea
            value={text}
            rows={3}
            spellCheck={false}
            placeholder={t('settings.memory.textPlaceholder')}
            onChange={(event) => setText(event.target.value)}
          />
        </label>
        {formError ? <p className="ark-settings__hint ark-settings__hint--error">{formError}</p> : null}
        <div className="ark-workspace-switch__controls">
          <label className="ark-settings__hint">
            {t('settings.memory.tagsLabel')}
            <input
              value={tags}
              spellCheck={false}
              autoComplete="off"
              placeholder={t('settings.memory.tagsPlaceholder')}
              onChange={(event) => setTags(event.target.value)}
            />
          </label>
          <label className="ark-settings__hint">
            {t('settings.memory.sourceLabel')}
            <input
              value={source}
              spellCheck={false}
              autoComplete="off"
              onChange={(event) => setSource(event.target.value)}
            />
          </label>
          <Button size="sm" disabled={saving} onClick={() => void create()}>
            {saving ? t('common.saving') : t('settings.memory.create')}
          </Button>
        </div>
        <p className="ark-settings__hint">{t('settings.memory.injectHint')}</p>
      </div>

      {api.status === 'loading' || api.status === 'idle' ? <Skeleton rows={2} /> : null}
      {api.status === 'error' ? (
        <LoadError message={api.error ?? t('settings.memory.loadFailed')} onRetry={() => void api.refresh()} />
      ) : null}
      {api.status === 'ready' && api.entries.length === 0 ? (
        <EmptyState title={t('settings.memory.emptyTitle')} hint={t('settings.memory.emptyHint')} />
      ) : null}
      {api.entries.length > 0 ? (
        <div className="ark-settings__list">
          {api.entries.map((entry) => (
            <MemoryCard key={entry.id} entry={entry} onDelete={(id) => void remove(id)} onUpdate={(id, text2, tags2, source2) => void update(id, text2, tags2, source2)} />
          ))}
        </div>
      ) : null}
      {api.status === 'ready' && api.diagnostics == null ? (
        <Notice title={t('settings.memory.diagnosticsUnavailable')} />
      ) : null}
    </SettingsSection>
  )
}
