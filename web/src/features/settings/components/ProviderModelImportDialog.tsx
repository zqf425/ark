/**
 * Batch model-import dialog for a provider (adapted from smart-ark).
 *
 * Source rows come from the fetched catalog plus a paste box for endpoints
 * without a `/models` route. Already-present ids are marked and deselected by
 * default so confirming imports only what is new. Pure row derivation lives in
 * ProviderCatalog.ts; this component only owns selection state.
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ProviderModelSummary } from '../../../shared/contracts'
import { PROVIDER_MODELS_MAX } from '../../../shared/contracts/web-only'
import {
  buildImportCandidates,
  defaultImportSelection,
  type ImportCandidate
} from './ProviderCatalog'
import type { ProviderDraft } from './ProviderDraft'

export function ProviderModelImportDialog({
  catalog,
  draft,
  onCancel,
  onConfirm
}: {
  catalog: ProviderModelSummary[]
  draft: ProviderDraft
  onCancel: () => void
  onConfirm: (ids: string[]) => void
}): JSX.Element {
  const { t } = useI18n()
  const panelRef = useRef<HTMLDivElement | null>(null)
  const [pastedText, setPastedText] = useState('')
  const [query, setQuery] = useState('')
  const [hideExisting, setHideExisting] = useState(true)
  const [selected, setSelected] = useState<Set<string>>(() =>
    defaultImportSelection(buildImportCandidates(catalog, '', draft))
  )

  const rows = useMemo(
    () => buildImportCandidates(catalog, pastedText, draft),
    [catalog, pastedText, draft]
  )

  // Pasted rows are new candidates: pre-check the fresh ones so confirming a
  // paste imports it, without touching choices the user already made.
  const appendPaste = (text: string): void => {
    const fresh = buildImportCandidates(catalog, text, draft)
      .filter((row) => !row.alreadyPresent && !selected.has(row.id))
    if (fresh.length > 0) {
      setSelected((previous) => new Set([...previous, ...fresh.map((row) => row.id)]))
    }
    setPastedText(text)
  }

  const normalizedQuery = query.trim().toLowerCase()
  const visible = rows.filter((row) => {
    if (hideExisting && row.alreadyPresent) return false
    if (normalizedQuery && !row.id.toLowerCase().includes(normalizedQuery)) return false
    return true
  })

  const checked = (row: ImportCandidate): boolean => selected.has(row.id)
  // Count the intersection with the real rows: pasted prefixes accumulate in
  // `selected` while typing and must not inflate the labels.
  const chosen = rows.filter((row) => selected.has(row.id))
  const overCap = chosen.length > PROVIDER_MODELS_MAX
  const toggle = (id: string): void => {
    const next = new Set(selected)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    setSelected(next)
  }
  const allVisibleSelected = visible.length > 0 && visible.every((row) => checked(row))
  const toggleAllVisible = (): void => {
    const next = new Set(selected)
    for (const row of visible) {
      if (allVisibleSelected) next.delete(row.id)
      else next.add(row.id)
    }
    setSelected(next)
  }

  const confirm = (): void => {
    onConfirm(chosen.map((row) => row.id))
  }

  useEffect(() => {
    panelRef.current?.focus()
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (event.key === 'Escape') onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onCancel])

  return (
    <div className="ark-importdialog" role="dialog" aria-modal="true" aria-label={t('settings.providerImport.title')}>
      <div className="ark-importdialog__panel" ref={panelRef} tabIndex={-1}>
        <header className="ark-importdialog__head">
          <div>
            <strong>{t('settings.providerImport.title')}</strong>
            <p className="ark-settings__hint">
              {t('settings.providerImport.subtitle', { total: rows.length, present: rows.filter((row) => row.alreadyPresent).length })}
            </p>
          </div>
          <Button variant="ghost" size="sm" onClick={onCancel} aria-label={t('common.close')}>
            ×
          </Button>
        </header>

        <div className="ark-importdialog__filters">
          <input
            className="ark-field__input"
            value={query}
            spellCheck={false}
            placeholder={t('settings.providerImport.searchPlaceholder')}
            aria-label={t('settings.providerImport.searchPlaceholder')}
            onChange={(event) => setQuery(event.target.value)}
          />
          <label className="ark-check">
            <input
              type="checkbox"
              checked={hideExisting}
              onChange={(event) => setHideExisting(event.target.checked)}
            />
            {t('settings.providerImport.hideExisting', { count: rows.filter((row) => row.alreadyPresent).length })}
          </label>
          <textarea
            className="ark-field__area"
            rows={3}
            value={pastedText}
            spellCheck={false}
            placeholder={t('settings.providerImport.pastePlaceholder')}
            aria-label={t('settings.providerImport.pastePlaceholder')}
            onChange={(event) => appendPaste(event.target.value)}
          />
        </div>

        <div className="ark-importdialog__list">
          {visible.length === 0 ? (
            <p className="ark-settings__hint">
              {rows.length === 0
                ? t('settings.providerImport.nothingFetched')
                : t('settings.providerImport.nothingMatches')}
            </p>
          ) : (
            <ul className="ark-importdialog__rows">
              {visible.map((row) => (
                <li key={row.id}>
                  <label className={`ark-importdialog__row${checked(row) ? ' is-checked' : ''}`}>
                    <input type="checkbox" checked={checked(row)} onChange={() => toggle(row.id)} />
                    <span className="ark-importdialog__row-id">{row.id}</span>
                    {row.label ? <span className="ark-settings__hint">{row.label}</span> : null}
                    {row.alreadyPresent ? (
                      <span className="ark-chip ark-chip--muted">{t('settings.providerImport.alreadyPresent')}</span>
                    ) : null}
                  </label>
                </li>
              ))}
            </ul>
          )}
        </div>

        <footer className="ark-importdialog__foot">
          <span className="ark-settings__hint">
            {t('settings.providerImport.selectedCount', { count: chosen.length })}
            {overCap ? ` ${t('settings.providerImport.overCap', { max: PROVIDER_MODELS_MAX })}` : ''}
          </span>
          <div className="ark-settings__row">
            <Button size="sm" disabled={visible.length === 0} onClick={toggleAllVisible}>
              {allVisibleSelected
                ? t('settings.providerImport.clearVisible')
                : t('settings.providerImport.selectAllVisible', { count: visible.length })}
            </Button>
            <Button size="sm" onClick={onCancel}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" size="sm" disabled={chosen.length === 0 || overCap} onClick={confirm}>
              {t('settings.providerImport.confirm', { count: selected.size })}
            </Button>
          </div>
        </footer>
      </div>
    </div>
  )
}
