/**
 * Provider model-catalog form helpers.
 *
 * Kept outside the React component so JSON/header parsing and the rule that
 * remote metadata is applied only when explicit stay unit-testable.
 */
import type {
  ProviderModelCatalogRequest,
  ProviderModelSummary
} from '../../../shared/contracts'
import { translateNow } from '../../../shared/i18n/current'
import type { ProviderDraft } from './ProviderDraft'

export type BuiltHeaders = { ok: true; headers: Record<string, string> } | { ok: false; error: string }

/** Parse the write-only custom headers field into a request-ready map. */
export function parseProviderHeaders(headersText: string): BuiltHeaders {
  const raw = headersText.trim()
  if (!raw) return { ok: true, headers: {} }
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { ok: false, error: translateNow('settings.providerForm.error.headersNotObject') }
    }
    const headers: Record<string, string> = {}
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      headers[key] = typeof value === 'string' ? value : String(value)
    }
    return { ok: true, headers }
  } catch {
    return { ok: false, error: translateNow('settings.providerForm.error.headersNotJson') }
  }
}

/** Apply only remote metadata that the catalog explicitly returned. */
export function applyCatalogModel(draft: ProviderDraft, model: ProviderModelSummary): ProviderDraft {
  return {
    ...draft,
    model: model.id,
    context_window_tokens: model.context_window_tokens?.toString() ?? '',
    supports_vision: model.supports_vision ?? null
  }
}

/** Build a model-catalog request without requiring the user to enter a model first. */
export function buildCatalogRequest(
  draft: ProviderDraft,
  editing: boolean
): { ok: true; request: ProviderModelCatalogRequest } | { ok: false; error: string } {
  const baseUrl = draft.base_url.trim()
  if (!baseUrl) return { ok: false, error: translateNow('settings.providerForm.error.catalogNeedsBaseUrl') }
  const headers = parseProviderHeaders(draft.headers_text)
  if (!headers.ok) return headers
  const apiKey = draft.api_key.trim()
  if (!editing && !apiKey) return { ok: false, error: translateNow('settings.providerForm.error.catalogNeedsApiKey') }
  return {
    ok: true,
    request: {
      ...(editing && draft.id.trim() ? { provider_id: draft.id.trim() } : {}),
      connection: {
        base_url: baseUrl,
        ...(apiKey ? { api_key: apiKey } : {}),
        endpoint_format: draft.endpoint_format,
        headers: headers.headers
      }
    }
  }
}

/** Build a concise option label without representing unknown capabilities as false. */
export function catalogLabel(model: ProviderModelSummary): string {
  const details: string[] = []
  if (model.context_window_tokens) details.push(`${model.context_window_tokens} tokens`)
  if (model.max_output_tokens) {
    details.push(translateNow('settings.providerCatalog.maxOutput', { value: model.max_output_tokens }))
  }
  if (model.supports_vision === true) details.push(translateNow('settings.providerCatalog.visionSupported'))
  if (model.supports_vision === false) details.push(translateNow('settings.providerCatalog.visionUnsupported'))
  return [model.label ?? model.id, details.join(' · ')].filter(Boolean).join(' — ')
}

// ---------------------------------------------------------------------------
// Batch model import (adapted from smart-ark's import dialog, sized to arkd)
// ---------------------------------------------------------------------------

/** One selectable row of the import dialog. */
export type ImportCandidate = {
  id: string
  /** Catalog metadata label, when the row came from the fetched catalog. */
  label: string | null
  /** Already the default model or inside the saved inventory. */
  alreadyPresent: boolean
}

/** Parse a paste box: one id per line, trimmed, empties and duplicates dropped. */
export function parsePastedModelIds(text: string): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const line of text.split('\n')) {
    const id = line.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    out.push(id)
  }
  return out
}

/**
 * Build the checklist rows: every catalog model plus pasted ids, marked
 * already-present when they equal the default model or sit in the inventory.
 */
export function buildImportCandidates(
  catalog: ProviderModelSummary[],
  pastedText: string,
  draft: Pick<ProviderDraft, 'model' | 'models'>
): ImportCandidate[] {
  const present = new Set<string>([draft.model.trim(), ...draft.models.map((id) => id.trim())].filter(Boolean))
  const rows: ImportCandidate[] = []
  const seen = new Set<string>()
  for (const model of catalog) {
    if (seen.has(model.id)) continue
    seen.add(model.id)
    rows.push({ id: model.id, label: catalogLabel(model), alreadyPresent: present.has(model.id) })
  }
  for (const id of parsePastedModelIds(pastedText)) {
    if (seen.has(id)) continue
    seen.add(id)
    rows.push({ id, label: null, alreadyPresent: present.has(id) })
  }
  return rows
}

/** Default selection: every row that is not already in the provider. */
export function defaultImportSelection(rows: ImportCandidate[]): Set<string> {
  return new Set(rows.filter((row) => !row.alreadyPresent).map((row) => row.id))
}

/**
 * Fold a confirmed selection into the draft. The default model is only
 * backfilled when empty (the form requires one), and the inventory keeps the
 * dialog's order.
 */
export function mergeImportedModels(
  draft: ProviderDraft,
  selectedIds: readonly string[]
): ProviderDraft {
  // The backfilled default must not also sit in the inventory: the store
  // normalizes duplicates of `model` away, so mirror that here.
  const defaultModel = draft.model.trim() || selectedIds.find((id) => id.trim())?.trim() || ''
  const models = [...draft.models]
  for (const raw of selectedIds) {
    const id = raw.trim()
    if (!id || id === defaultModel || models.includes(id)) continue
    models.push(id)
  }
  return { ...draft, models, model: defaultModel }
}
