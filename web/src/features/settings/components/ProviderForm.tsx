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
import { translateNow } from '../../../shared/i18n/current'
import {
  ENDPOINT_FORMATS,
  type EndpointFormat,
  type ProviderEntry,
  type ProviderModelCatalogRequest,
  type ProviderModelCatalogResponse,
  type ProviderModelSummary,
  type ProviderView
} from '../../../shared/contracts'
import { CheckField, Field, SelectField, TextField } from './Field'
import { applyCatalogModel, buildCatalogRequest, catalogLabel, parseProviderHeaders } from './ProviderCatalog'
import { Notice } from './SectionShell'

/**
 * Endpoint ids are wire values, so only the parenthesised path hint is prose;
 * the path itself is the literal URL the runtime appends.
 */
const FORMAT_LABELS: Record<EndpointFormat, string> = {
  openai_chat: 'openai_chat (/chat/completions)',
  openai_responses: 'openai_responses (/responses)',
  anthropic_messages: 'anthropic_messages (/v1/messages)'
}

export type ProviderDraft = {
  id: string
  label: string
  base_url: string
  endpoint_format: EndpointFormat
  model: string
  api_key: string
  enabled: boolean
  headers_text: string
  max_tokens: string
  temperature: string
  context_window_tokens: string
  supports_vision: boolean | null
}

export function emptyDraft(): ProviderDraft {
  return {
    id: '',
    label: '',
    base_url: '',
    endpoint_format: 'openai_chat',
    model: '',
    api_key: '',
    enabled: true,
    headers_text: '',
    max_tokens: '',
    temperature: '',
    context_window_tokens: '',
    supports_vision: null
  }
}

export function draftFromView(view: ProviderView): ProviderDraft {
  const format = (ENDPOINT_FORMATS as readonly string[]).includes(view.endpoint_format)
    ? (view.endpoint_format as EndpointFormat)
    : 'openai_chat'
  return {
    id: view.id,
    label: view.label ?? '',
    base_url: view.base_url,
    endpoint_format: format,
    model: view.model,
    // Never prefilled: the runtime only ever reports a hint, not the key.
    api_key: '',
    enabled: view.enabled,
    headers_text: '',
    max_tokens: '',
    temperature: '',
    context_window_tokens: view.context_window_tokens?.toString() ?? '',
    supports_vision: view.supports_vision ?? null
  }
}

export type BuiltEntry = { ok: true; entry: ProviderEntry } | { ok: false; error: string }

/** Validate locally, then map onto the wire shape. Numbers stay strings in the form. */
export function buildEntry(draft: ProviderDraft, editing: boolean): BuiltEntry {
  const id = draft.id.trim()
  const baseUrl = draft.base_url.trim()
  const model = draft.model.trim()
  if (!id) return { ok: false, error: translateNow('settings.providerForm.error.idRequired') }
  if (!baseUrl) return { ok: false, error: translateNow('settings.providerForm.error.baseUrlRequired') }
  if (!model) return { ok: false, error: translateNow('settings.providerForm.error.modelRequired') }

  const entry: ProviderEntry = {
    id,
    base_url: baseUrl,
    endpoint_format: draft.endpoint_format,
    model,
    enabled: draft.enabled,
    headers: {}
  }
  const label = draft.label.trim()
  if (label) entry.label = label

  const apiKey = draft.api_key.trim()
  if (apiKey) entry.api_key = apiKey
  else if (!editing) return { ok: false, error: translateNow('settings.providerForm.error.apiKeyRequired') }

  const headers = parseProviderHeaders(draft.headers_text)
  if (!headers.ok) return headers
  entry.headers = headers.headers

  if (draft.max_tokens.trim()) {
    const parsed = Number(draft.max_tokens.trim())
    if (!Number.isFinite(parsed) || parsed <= 0 || !Number.isInteger(parsed)) {
      return { ok: false, error: translateNow('settings.providerForm.error.maxTokensPositive') }
    }
    entry.max_tokens = parsed
  }

  if (draft.temperature.trim()) {
    const parsed = Number(draft.temperature.trim())
    if (!Number.isFinite(parsed) || parsed < 0 || parsed > 2) {
      return { ok: false, error: translateNow('settings.providerForm.error.temperatureRange') }
    }
    entry.temperature = parsed
  }

  if (draft.context_window_tokens.trim()) {
    const parsed = Number(draft.context_window_tokens.trim())
    if (!Number.isSafeInteger(parsed) || parsed <= 0) {
      return { ok: false, error: translateNow('settings.providerForm.error.contextWindowPositive') }
    }
    entry.context_window_tokens = parsed
  }
  if (draft.supports_vision !== null) entry.supports_vision = draft.supports_vision

  return { ok: true, entry }
}

export type ProviderFormProps = {
  editing: boolean
  initial: ProviderDraft
  busy: boolean
  onCancel: () => void
  onSubmit: (entry: ProviderEntry, makeDefault: boolean) => Promise<void>
  onTest: (entry: ProviderEntry) => Promise<void>
  onDiscover: (request: ProviderModelCatalogRequest) => Promise<ProviderModelCatalogResponse>
}

export function ProviderForm({ editing, initial, busy, onCancel, onSubmit, onTest, onDiscover }: ProviderFormProps): JSX.Element {
  const { t } = useI18n()
  const [draft, setDraft] = useState<ProviderDraft>(initial)
  const [advanced, setAdvanced] = useState(false)
  const [makeDefault, setMakeDefault] = useState(!editing)
  const [error, setError] = useState<string | null>(null)
  const [catalog, setCatalog] = useState<ProviderModelSummary[]>([])
  const [catalogError, setCatalogError] = useState<string | null>(null)

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

  const discover = async (): Promise<void> => {
    const built = buildCatalogRequest(draft, editing)
    if (!built.ok) {
      setCatalogError(built.error)
      return
    }
    setCatalogError(null)
    try {
      const response = await onDiscover(built.request)
      setCatalog(response.models)
      if (response.models.length === 0) setCatalogError(t('settings.providers.catalogEmpty'))
    } catch (thrown) {
      setCatalog([])
      setCatalogError(thrown instanceof Error ? thrown.message : t('settings.providers.catalogQueryFailedShort'))
    }
  }

  const catalogLabels = Object.fromEntries(catalog.map((model) => [model.id, catalogLabel(model)])) as Record<string, string>

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
        {catalog.length > 0 ? (
          <Field
            label={t('settings.providerForm.catalogLabel')}
            htmlFor="provider-catalog-model"
            hint={t('settings.providerForm.catalogHint')}
          >
            <select
              id="provider-catalog-model"
              className="ark-field__select"
              value={catalog.some((item) => item.id === draft.model) ? draft.model : ''}
              disabled={busy}
              onChange={(event) => {
                const model = catalog.find((item) => item.id === event.target.value)
                if (model) setDraft((current) => applyCatalogModel(current, model))
              }}
            >
              <option value="">{t('settings.providerForm.catalogPlaceholder')}</option>
              {catalog.map((model) => <option key={model.id} value={model.id}>{catalogLabels[model.id] ?? model.id}</option>)}
            </select>
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
        <Button size="sm" disabled={busy} onClick={() => void discover()}>
          {busy ? t('settings.providerForm.querying') : t('settings.providerForm.queryModels')}
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
