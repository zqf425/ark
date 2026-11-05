/**
 * Provider create/edit form.
 *
 * Pure form state: it never calls the api itself. The parent owns `busy` and
 * folds the returned `ProviderEntry` into a request. The API key field is
 * write-only — the runtime never returns it, so leaving it blank on edit means
 * "keep the stored key".
 */
import { useState } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import {
  ENDPOINT_FORMATS,
  type EndpointFormat,
  type ProviderEntry,
  type ProviderModelCatalogRequest,
  type ProviderModelCatalogResponse,
  type ProviderModelSummary
} from '../../../shared/contracts'
import { CheckField, Field, SelectField, TextField } from './Field'
import {
  buildCatalogRequest,
  mergeImportedModels,
  updateInventoryItem
} from './ProviderCatalog'
import { ProviderModelImportDialog } from './ProviderModelImportDialog'
import { Notice } from './SectionShell'
import { buildEntry, type ProviderDraft } from './ProviderDraft'

/**
 * Endpoint ids are wire values, so only the parenthesised path hint is prose;
 * the path itself is the literal URL the runtime appends.
 */
const FORMAT_LABELS: Record<EndpointFormat, string> = {
  openai_chat: 'openai_chat (/chat/completions)',
  openai_responses: 'openai_responses (/responses)',
  anthropic_messages: 'anthropic_messages (/v1/messages)'
}

export type ProviderFormProps = {
  editing: boolean
  initial: ProviderDraft
  busy: boolean
  /** True only while the model-catalog query runs; never locks the form. */
  catalogBusy: boolean
  onCancel: () => void
  onSubmit: (entry: ProviderEntry, makeDefault: boolean) => Promise<void>
  onTest: (entry: ProviderEntry) => Promise<void>
  onDiscover: (request: ProviderModelCatalogRequest) => Promise<ProviderModelCatalogResponse>
}

export function ProviderForm({ editing, initial, busy, catalogBusy, onCancel, onSubmit, onTest, onDiscover }: ProviderFormProps): JSX.Element {
  const { t } = useI18n()
  const [draft, setDraft] = useState<ProviderDraft>(initial)
  const [advanced, setAdvanced] = useState(false)
  const [makeDefault, setMakeDefault] = useState(!editing)
  const [error, setError] = useState<string | null>(null)
  const [catalog, setCatalog] = useState<ProviderModelSummary[]>([])
  const [catalogError, setCatalogError] = useState<string | null>(null)
  const [importing, setImporting] = useState(false)
  const patch = <K extends keyof ProviderDraft>(key: K, value: ProviderDraft[K]): void => {
    setDraft((current) => ({ ...current, [key]: value }))
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
    <div className="ark-card ark-card--editing">
      <div className="ark-card__meta">
        <strong>{editing ? t('settings.providerForm.editTitle', { id: initial.id }) : t('settings.providerForm.createTitle')}</strong>
      </div>

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
        <TextField
          id="provider-label"
          label={t('settings.providerForm.labelLabel')}
          value={draft.label}
          disabled={busy}
          placeholder={t('settings.providerForm.labelPlaceholder')}
          onChange={(value) => patch('label', value)}
        />
        <TextField
          id="provider-base-url"
          label="base_url"
          value={draft.base_url}
          mono
          disabled={busy}
          placeholder="https://api.deepseek.com"
          onChange={(value) => patch('base_url', value)}
        />
        <SelectField<EndpointFormat>
          id="provider-format"
          label="endpoint_format"
          value={draft.endpoint_format}
          options={ENDPOINT_FORMATS}
          labels={FORMAT_LABELS}
          disabled={busy}
          onChange={(value) => patch('endpoint_format', value)}
        />
        <TextField
          id="provider-model"
          label={t('settings.providerForm.modelLabel')}
          value={draft.model}
          mono
          disabled={busy}
          placeholder={t('settings.providerForm.modelPlaceholder')}
          onChange={(value) => patch('model', value)}
        />
        {draft.models.length > 0 ? (
          <Field
            label={t('settings.providerForm.inventoryLabel')}
            htmlFor="provider-model-inventory"
            hint={t('settings.providerForm.inventoryHint')}
          >
            <div id="provider-model-inventory" className="ark-importdialog__rows">
              {draft.models.map((item) => (
                <div key={item.id} className="ark-importdialog__row">
                  <span className="ark-importdialog__row-id">{item.id}</span>
                  <input
                    className="ark-field__input ark-provider-model__context"
                    type="number"
                    min={1}
                    placeholder={t('settings.providerForm.inventoryContextPlaceholder')}
                    aria-label={t('settings.providerForm.inventoryContextLabel', { model: item.id })}
                    value={item.context_window_tokens ?? ''}
                    disabled={busy}
                    onChange={(event) => {
                      const raw = event.target.value.trim()
                      if (raw === '') {
                        setDraft((current) => updateInventoryItem(current, item.id, { context_window_tokens: undefined }))
                        return
                      }
                      const parsed = Number(raw)
                      if (!Number.isSafeInteger(parsed) || parsed <= 0) return
                      setDraft((current) => updateInventoryItem(current, item.id, { context_window_tokens: parsed }))
                    }}
                  />
                  <label className="ark-check">
                    <input
                      type="checkbox"
                      checked={item.supports_vision === true}
                      disabled={busy}
                      aria-label={t('settings.providerForm.inventoryVisionLabel', { model: item.id })}
                      onChange={(event) =>
                        setDraft((current) => updateInventoryItem(current, item.id, { supports_vision: event.target.checked }))
                      }
                    />
                    {t('settings.providerForm.inventoryVision')}
                  </label>
                  <button
                    type="button"
                    className="ark-importdialog__chip-remove"
                    aria-label={t('settings.providerForm.inventoryRemove', { model: item.id })}
                    disabled={busy}
                    onClick={() =>
                      setDraft((current) => ({
                        ...current,
                        models: current.models.filter((seen) => seen.id !== item.id)
                      }))
                    }
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          </Field>
        ) : null}
        <TextField
          id="provider-api-key"
          label="api_key"
          type="password"
          value={draft.api_key}
          disabled={busy}
          placeholder={
            editing ? t('settings.providerForm.apiKeyPlaceholderKeep') : t('settings.providerForm.apiKeyPlaceholderNew')
          }
          hint={editing ? t('settings.providerForm.apiKeyHintEditing') : t('settings.providerForm.apiKeyHintCreating')}
          onChange={(value) => patch('api_key', value)}
        />
      </div>

      <div className="ark-card__foot">
        <CheckField
          label={t('settings.providerForm.enabledLabel')}
          checked={draft.enabled}
          disabled={busy}
          onChange={(value) => patch('enabled', value)}
        />
        {!editing ? (
          <CheckField
            label={t('settings.providerForm.makeDefaultLabel')}
            checked={makeDefault}
            disabled={busy}
            onChange={setMakeDefault}
          />
        ) : null}
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
          <TextField
            id="provider-context-window"
            label={t('settings.providerForm.contextWindowLabel')}
            value={draft.context_window_tokens}
            disabled={busy}
            placeholder="128000"
            type="number"
            hint={t('settings.providerForm.contextWindowHint')}
            onChange={(value) => patch('context_window_tokens', value)}
          />
          <CheckField
            label={t('settings.providerForm.visionLabel')}
            checked={draft.supports_vision === true}
            disabled={busy}
            onChange={(value) => patch('supports_vision', value)}
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

      <div className="ark-settings__actions">
        <Button
          variant="primary"
          size="sm"
          disabled={busy}
          onClick={() => {
            const entry = resolve()
            if (entry) void onSubmit(entry, makeDefault)
          }}
        >
          {busy ? t('common.saving') : t('common.save')}
        </Button>
        <Button
          size="sm"
          disabled={busy}
          onClick={() => {
            const entry = resolve()
            if (entry) void onTest(entry)
          }}
        >
          {t('common.test')}
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </div>
  )
}
