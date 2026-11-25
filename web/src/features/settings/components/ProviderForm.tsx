/**
 * Provider create/edit form.
 *
 * Pure form state: it never calls the api itself. The parent owns `busy` and
 * folds the returned `ProviderEntry` into a request. Editing a saved provider
 * seeds this form with the full entry (including the stored api key) from the
 * explicit edit read; the list view itself stays redacted. The label field is
 * gone from the UI but an existing label is kept in the draft so saving does
 * not silently drop it.
 *
 * Saving never promotes the entry to the default provider (aligned with
 * the provider list): a default switch silently re-routes every session that follows
 * the default, so it stays a deliberate action on the list row or in
 * Settings -> 通用, never a side effect of adding a provider.
 */
import { useState } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import {
  ENDPOINT_FORMATS,
  type ProviderEntry,
  type ProviderModelCatalogRequest,
  type ProviderModelCatalogResponse,
  type ProviderModelSummary
} from '../../../shared/contracts'
import { CheckField, Field, TextField } from './Field'
import { FORMAT_NAMES, requestUrl } from './ProviderDetail'
import {
  appendManualModel,
  buildCatalogRequest,
  mergeImportedModels,
  updateInventoryItem
} from './ProviderCatalog'
import { ProviderModelImportDialog } from './ProviderModelImportDialog'
import { Notice } from './SectionShell'
import { buildEntry, type ProviderDraft } from './ProviderDraft'

/** Normalize a context-window input to a positive integer or "unknown". */
function parseContextInput(raw: string): number | undefined {
  const trimmed = raw.trim()
  if (!trimmed) return undefined
  const parsed = Number(trimmed)
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : undefined
}

type ModelRowProps = {
  id: string
  contextValue: string
  vision: boolean | null
  isDefault?: boolean
  busy: boolean
  /** Only the default model owns an editable id; imported rows show a read-only
   *  id because their ids come from the catalog. This keeps one row shape for
   *  every model instead of a separate input above the list. */
  onIdChange?: (value: string) => void
  idPlaceholder?: string
  onContextChange: (raw: string) => void
  onVisionChange: (value: boolean) => void
  onRemove?: () => void
}

/**
 * One model row shared by the default model and every imported model, so both
 * show the same id + context window + vision controls. The default row's values
 * live at entry level (`draft.context_window_tokens` / `supports_vision`);
 * imported rows live in `draft.models`. Each row edits only its own values:
 * there is no cross-row override, which is why the advanced section no longer
 * duplicates the window/vision fields.
 */
function ModelRow({ id, contextValue, vision, isDefault = false, busy, onIdChange, idPlaceholder, onContextChange, onVisionChange, onRemove }: ModelRowProps): JSX.Element {
  const { t } = useI18n()
  return (
    <div className="ark-importdialog__row">
      {onIdChange ? (
        <input
          id="provider-model"
          className="ark-field__input ark-provider-model__id"
          value={id}
          disabled={busy}
          spellCheck={false}
          autoComplete="off"
          placeholder={idPlaceholder}
          aria-label={t('settings.providerForm.defaultModelLabel')}
          onChange={(event) => onIdChange(event.target.value)}
        />
      ) : (
        <span className="ark-importdialog__row-id ark-provider-model__id">{id}</span>
      )}
      {isDefault ? (
        <span className="ark-chip ark-chip--muted">{t('settings.providerForm.defaultModelBadge')}</span>
      ) : null}
      <input
        className="ark-field__input ark-provider-model__context"
        type="number"
        min={1}
        placeholder={t('settings.providerForm.inventoryContextPlaceholder')}
        aria-label={t('settings.providerForm.inventoryContextLabel', { model: id })}
        value={contextValue}
        disabled={busy}
        onChange={(event) => onContextChange(event.target.value)}
      />
      <label className="ark-check">
        <input
          type="checkbox"
          checked={vision === true}
          disabled={busy}
          aria-label={t('settings.providerForm.inventoryVisionLabel', { model: id })}
          onChange={(event) => onVisionChange(event.target.checked)}
        />
        {t('settings.providerForm.inventoryVision')}
      </label>
      {onRemove ? (
        <button
          type="button"
          className="ark-importdialog__chip-remove"
          aria-label={t('settings.providerForm.inventoryRemove', { model: id })}
          disabled={busy}
          onClick={onRemove}
        >
          ×
        </button>
      ) : null}
    </div>
  )
}

export type ProviderFormProps = {
  editing: boolean
  initial: ProviderDraft
  busy: boolean
  /** True only while the model-catalog query runs; never locks the form. */
  catalogBusy: boolean
  onCancel: () => void
  onSubmit: (entry: ProviderEntry) => Promise<void>
  onTest: (entry: ProviderEntry) => Promise<void>
  onDiscover: (request: ProviderModelCatalogRequest) => Promise<ProviderModelCatalogResponse>
}

export function ProviderForm({ editing, initial, busy, catalogBusy, onCancel, onSubmit, onTest, onDiscover }: ProviderFormProps): JSX.Element {
  const { t } = useI18n()
  const [draft, setDraft] = useState<ProviderDraft>(initial)
  const [advanced, setAdvanced] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [catalog, setCatalog] = useState<ProviderModelSummary[]>([])
  const [catalogError, setCatalogError] = useState<string | null>(null)
  const [importing, setImporting] = useState(false)
  /** Pending hand-entered model id; empty input is a no-op on submit. */
  const [manualModel, setManualModel] = useState('')
  const patch = <K extends keyof ProviderDraft>(key: K, value: ProviderDraft[K]): void => {
    setDraft((current) => ({ ...current, [key]: value }))
  }

  /** Append the pending manual id; a duplicate or default-equal input keeps
   *  the text so the user can correct it instead of silently dropping it. */
  const commitManualModel = (): void => {
    const id = manualModel.trim()
    if (!id) return
    const next = appendManualModel(draft, id)
    setDraft(next)
    // The helper returns the same draft reference on a no-op.
    if (next !== draft) setManualModel('')
  }

  const resolve = (): ProviderEntry | null => {
    const built = buildEntry(draft, editing)
    if (!built.ok) {
      setError(built.error)
      return null
    }
    setError(null)
    return built.entry
  }

  // One merged entry point: fetch the remote catalog, then open the picker
  // over it. A failed or empty fetch keeps the dialog closed and reports
  // inline, so the operator never confirms against a stale list.
  const openImport = async (): Promise<void> => {
    const built = buildCatalogRequest(draft, editing)
    if (!built.ok) {
      setCatalogError(built.error)
      return
    }
    setCatalogError(null)
    try {
      const response = await onDiscover(built.request)
      setCatalog(response.models)
      if (response.models.length === 0) {
        setCatalogError(t('settings.providers.catalogEmpty'))
        return
      }
      setImporting(true)
    } catch (thrown) {
      setCatalog([])
      setCatalogError(thrown instanceof Error ? thrown.message : t('settings.providers.catalogQueryFailedShort'))
    }
  }

  return (
    <div className="ark-card ark-provider-form">
      <div className="ark-provider-form__crumb">
        <Button variant="ghost" size="sm" disabled={busy} onClick={onCancel}>← {t('settings.providerForm.back')}</Button>
        <strong>{editing ? t('settings.providerForm.editTitle', { id: initial.id }) : t('settings.providerForm.createTitle')}</strong>
      </div>

      <h3 className="ark-provider-form__step"><em>1</em>{t('settings.providerForm.step.basic')}</h3>
      <div className="ark-settings__grid">
        <TextField
          id="provider-id"
          label={t('settings.providerForm.idLabel')}
          value={draft.id}
          mono
          disabled={editing || busy}
          placeholder={t('settings.providerForm.idPlaceholder')}
          hint={editing ? t('settings.providerForm.idHintEditing') : t('settings.providerForm.idHintCreating')}
          onChange={(value) => patch('id', value)}
        />
        <CheckField
          label={t('settings.providerForm.enabledLabel')}
          checked={draft.enabled}
          disabled={busy}
          onChange={(value) => patch('enabled', value)}
        />
      </div>

      <h3 className="ark-provider-form__step"><em>2</em>{t('settings.providerForm.step.connection')}</h3>
      <div className="ark-provider-seg" role="radiogroup" aria-label="endpoint_format">
        {ENDPOINT_FORMATS.map((format) => (
          <button
            key={format}
            type="button"
            role="radio"
            aria-checked={draft.endpoint_format === format}
            className={draft.endpoint_format === format ? 'is-on' : ''}
            disabled={busy}
            onClick={() => patch('endpoint_format', format)}
          >
            {FORMAT_NAMES[format]}
          </button>
        ))}
      </div>
      <div className="ark-settings__grid">
        <TextField
          id="provider-api-key"
          label="api_key"
          revealable
          value={draft.api_key}
          disabled={busy}
          placeholder={
            editing ? t('settings.providerForm.apiKeyPlaceholderKeep') : t('settings.providerForm.apiKeyPlaceholderNew')
          }
          hint={editing ? t('settings.providerForm.apiKeyHintEditing') : t('settings.providerForm.apiKeyHintCreating')}
          onChange={(value) => patch('api_key', value)}
        />
        <TextField
          id="provider-base-url"
          label="base_url"
          value={draft.base_url}
          mono
          disabled={busy}
          placeholder="https://api.deepseek.com"
          hint={draft.base_url.trim() ? `${t('settings.providerForm.requestPreview')}${requestUrl(draft.base_url, draft.endpoint_format)}` : undefined}
          onChange={(value) => patch('base_url', value)}
        />
      </div>

      <h3 className="ark-provider-form__step"><em>3</em>{t('settings.providerForm.step.models')}</h3>
      <div className="ark-settings__grid">
        <Field
          label={t('settings.providerForm.modelsLabel')}
          htmlFor="provider-model"
          hint={t('settings.providerForm.modelsHint')}
          className="ark-provider-form__models"
        >
          <div id="provider-models" className="ark-importdialog__rows">
            <ModelRow
              id={draft.model}
              contextValue={draft.context_window_tokens}
              vision={draft.supports_vision}
              isDefault
              busy={busy}
              onIdChange={(value) => patch('model', value)}
              idPlaceholder={t('settings.providerForm.modelPlaceholder')}
              onContextChange={(raw) => patch('context_window_tokens', raw)}
              onVisionChange={(value) => patch('supports_vision', value)}
            />
            {draft.models.map((item) => (
              <ModelRow
                key={item.id}
                id={item.id}
                contextValue={item.context_window_tokens?.toString() ?? ''}
                vision={item.supports_vision ?? null}
                busy={busy}
                onContextChange={(raw) => {
                  const parsed = parseContextInput(raw)
                  // Ignore an invalid intermediate value; keep the row's last
                  // valid window until the input is corrected or cleared.
                  if (raw.trim() !== '' && parsed === undefined) return
                  setDraft((current) =>
                    updateInventoryItem(current, item.id, { context_window_tokens: parsed })
                  )
                }}
                onVisionChange={(value) =>
                  setDraft((current) => updateInventoryItem(current, item.id, { supports_vision: value }))
                }
                onRemove={() =>
                  setDraft((current) => ({
                    ...current,
                    models: current.models.filter((seen) => seen.id !== item.id)
                  }))
                }
              />
            ))}
          </div>
          <input
            id="provider-model-manual"
            className="ark-field__input ark-provider-form__manual-model"
            value={manualModel}
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
            placeholder={t('settings.providerForm.addModelPlaceholder')}
            aria-label={t('settings.providerForm.addModel')}
            onChange={(event) => setManualModel(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                // Enter appends the pending id in place, so several models can
                // be typed in a row without reaching for the button.
                event.preventDefault()
                commitManualModel()
              }
            }}
          />
          <button
            type="button"
            className="ark-linkbtn"
            disabled={busy}
            aria-label={t('settings.providerForm.addModel')}
            onClick={commitManualModel}
          >
            {t('settings.providerForm.addModel')}
          </button>
        </Field>
      </div>

      <div className="ark-card__foot">
        <button type="button" className="ark-linkbtn" onClick={() => setAdvanced((value) => !value)}>
          {advanced ? t('settings.providerForm.advancedHide') : t('settings.providerForm.advancedShow')}
        </button>
        <Button size="sm" disabled={busy || catalogBusy} onClick={() => void openImport()}>
          {catalogBusy ? t('settings.providerForm.querying') : t('settings.providerForm.importModels')}
        </Button>
      </div>

      {advanced ? (
        <div className="ark-settings__grid">
          <Field
            label={t('settings.providerForm.headersLabel')}
            htmlFor="provider-headers"
            hint={t('settings.providerForm.headersHint')}
          >
            <textarea
              id="provider-headers"
              className="ark-field__area"
              value={draft.headers_text}
              disabled={busy}
              spellCheck={false}
              placeholder={'{\n  "X-Request-Source": "ark"\n}'}
              onChange={(event) => patch('headers_text', event.target.value)}
            />
          </Field>
          <TextField
            id="provider-max-tokens"
            label={t('settings.providerForm.maxTokensLabel')}
            value={draft.max_tokens}
            disabled={busy}
            placeholder="4096"
            type="number"
            hint={t('settings.providerForm.maxTokensHint')}
            onChange={(value) => patch('max_tokens', value)}
          />
          <TextField
            id="provider-temperature"
            label={t('settings.providerForm.temperatureLabel')}
            value={draft.temperature}
            disabled={busy}
            placeholder="0.3"
            type="number"
            hint={t('settings.providerForm.maxTokensHint')}
            onChange={(value) => patch('temperature', value)}
          />
        </div>
      ) : null}

      {importing ? (
        <ProviderModelImportDialog
          catalog={catalog}
          draft={draft}
          onCancel={() => setImporting(false)}
          onConfirm={(rows) => {
            setDraft((current) => mergeImportedModels(current, rows))
            setImporting(false)
          }}
        />
      ) : null}

      {error ? <Notice tone="error" title={t('settings.providerForm.validationFailed')}>{error}</Notice> : null}
      {catalogError ? <Notice tone="error" title={t('settings.providerForm.catalogFailed')}>{catalogError}</Notice> : null}

      <div className="ark-provider-form__footer">
        <Button
          size="sm"
          disabled={busy}
          onClick={() => {
            const entry = resolve()
            if (entry) void onTest(entry)
          }}
        >
          {t('settings.providers.testConnection')}
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
          {t('common.cancel')}
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={busy}
          onClick={() => {
            const entry = resolve()
            if (entry) void onSubmit(entry)
          }}
        >
          {busy ? t('common.saving') : t('common.save')}
        </Button>
      </div>
    </div>
  )
}
