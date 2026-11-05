/**
 * Provider draft state and the form→wire builder.
 *
 * Split from ProviderForm.tsx to respect the 1000-line file limit; the
 * component owns only React state, every mapping rule stays here and is
 * unit-testable without a renderer.
 */
import { translateNow } from '../../../shared/i18n/current'
import { parseProviderHeaders } from './ProviderCatalog'
import {
  ENDPOINT_FORMATS,
  type EndpointFormat,
  type ProviderEntry,
  type ProviderModelInventoryItem,
  type ProviderView
} from '../../../shared/contracts'
import { PROVIDER_MODELS_MAX } from '../../../shared/contracts/web-only'

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
  /** Imported model inventory; `model` stays the thread-visible default. */
  models: ProviderModelInventoryItem[]
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
    supports_vision: null,
    models: []
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
    supports_vision: view.supports_vision ?? null,
    models: view.models ?? []
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
    headers: {},
    models: []
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

  const models = new Map<string, ProviderModelInventoryItem>()
  for (const raw of draft.models) {
    const id = raw.id.trim()
    if (!id || id === model || models.has(id)) continue
    let context_window_tokens: number | undefined
    if (raw.context_window_tokens !== undefined) {
      const parsed = Number(raw.context_window_tokens)
      if (!Number.isSafeInteger(parsed) || parsed <= 0) {
        return { ok: false, error: translateNow('settings.providerForm.error.contextWindowPositive') }
      }
      context_window_tokens = parsed
    }
    models.set(id, { id, context_window_tokens, supports_vision: raw.supports_vision })
  }
  if (models.size > PROVIDER_MODELS_MAX) {
    return { ok: false, error: translateNow('settings.providerForm.error.tooManyModels', { max: PROVIDER_MODELS_MAX }) }
  }
  entry.models = [...models.values()]

  return { ok: true, entry }
}
