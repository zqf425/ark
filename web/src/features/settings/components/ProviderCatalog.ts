/**
 * Provider model-catalog form helpers.
 *
 * Kept outside the React component so JSON/header parsing and the rule that
 * remote metadata is applied only when explicit stay unit-testable.
 */
import type {
  ProviderModelCatalogRequest,
  ProviderModelInventoryItem,
  ProviderModelSummary
} from '../../../shared/contracts'
import { PROVIDER_DEFAULT_CONTEXT_WINDOW, PROVIDER_MODELS_MAX } from '../../../shared/contracts/web-only'
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

/**
 * One selectable row of the import dialog.
 *
 * Catalog-sourced rows carry the metadata the remote explicitly reported, so
 * confirming an import records the context window / vision flag together with
 * the id instead of losing it between dialog and form.
 */
export type ImportCandidate = {
  id: string
  /** Catalog metadata label, when the row came from the fetched catalog. */
  label: string | null
  /** Already the default model or inside the saved inventory. */
  alreadyPresent: boolean
  context_window_tokens?: number
  supports_vision?: boolean
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
  const present = new Set<string>([
    draft.model.trim(),
    ...draft.models.map((item) => item.id.trim())
  ].filter(Boolean))
  const rows: ImportCandidate[] = []
  const seen = new Set<string>()
  for (const model of catalog) {
    if (seen.has(model.id)) continue
    seen.add(model.id)
    // Probe the catalog's declared window; when the provider publishes none,
    // every imported row still gets a concrete 256K default (smart-ark parity)
    // instead of an empty field that silently falls back at runtime.
    const probed = model.context_window_tokens !== undefined
    const label = catalogLabel(model) + (probed ? '' : ` · ${translateNow('settings.providerImport.fallbackContext')}`)
    rows.push({
      id: model.id,
      label,
      alreadyPresent: present.has(model.id),
      context_window_tokens: model.context_window_tokens ?? PROVIDER_DEFAULT_CONTEXT_WINDOW,
      ...(model.supports_vision !== undefined
        ? { supports_vision: model.supports_vision }
        : {})
    })
  }
  for (const id of parsePastedModelIds(pastedText)) {
    if (seen.has(id)) continue
    seen.add(id)
    // Pasted ids have no catalog metadata at all: same 256K fallback, and the
    // dialog shows the fallback note so the value is never silently assumed.
    rows.push({
      id,
      label: translateNow('settings.providerImport.fallbackContext'),
      alreadyPresent: present.has(id),
      context_window_tokens: PROVIDER_DEFAULT_CONTEXT_WINDOW
    })
  }
  return rows
}

/**
 * Default selection: every row that is not already in the provider, capped at
 * the inventory ceiling.
 *
 * A catalog routinely returns hundreds of ids while `models` holds at most
 * `PROVIDER_MODELS_MAX`. Pre-selecting everything would open the dialog with a
 * permanently disabled confirm button, so the default is the first N selectable
 * rows and the user can still deselect or pick a different subset.
 */
export function defaultImportSelection(rows: ImportCandidate[]): Set<string> {
  return new Set(
    rows
      .filter((row) => !row.alreadyPresent)
      .slice(0, PROVIDER_MODELS_MAX)
      .map((row) => row.id)
  )
}

/**
 * Fold a confirmed selection into the draft. The default model is only
 * backfilled when empty (the form requires one), and the inventory keeps the
 * dialog's order. Catalog metadata rides along so the per-model context
 * window / vision flag survives the import without a second editing pass.
 */
export function mergeImportedModels(
  draft: ProviderDraft,
  selected: readonly ImportCandidate[]
): ProviderDraft {
  // The backfilled default must not also sit in the inventory: the store
  // normalizes duplicates of `model` away, so mirror that here.
  const defaultModel = draft.model.trim() || selected.find((row) => row.id.trim())?.id.trim() || ''
  const models = [...draft.models]
  let context_window_tokens = draft.context_window_tokens
  let supports_vision = draft.supports_vision
  for (const row of selected) {
    const id = row.id.trim()
    if (!id || id === defaultModel || models.some((seen) => seen.id === id)) continue
    models.push({
      id,
      ...(row.context_window_tokens !== undefined
        ? { context_window_tokens: row.context_window_tokens }
        : { context_window_tokens: PROVIDER_DEFAULT_CONTEXT_WINDOW }),
      ...(row.supports_vision !== undefined
        ? { supports_vision: row.supports_vision }
        : {})
    })
  }
  // The default model's metadata lives at entry level — that is what the
  // runtime reads for the stored model, and the store strips `model` from the
  // inventory — so a catalog row for the default model mirrors its metadata up.
  const defaultRow = selected.find((row) => row.id.trim() === defaultModel)
  if (defaultRow) {
    context_window_tokens = String(defaultRow.context_window_tokens ?? PROVIDER_DEFAULT_CONTEXT_WINDOW)
    if (defaultRow.supports_vision !== undefined) {
      supports_vision = defaultRow.supports_vision
    }
  }
  return { ...draft, models, model: defaultModel, context_window_tokens, supports_vision }
}

/**
 * Patch one inventory row's metadata in place (the id never changes).
 *
 * Fields the patch does not mention keep their current value; an explicit
 * `undefined` clears the field back to "unknown" — the key is dropped rather
 * than written as `null`, so the wire shape stays minimal.
 */
export function updateInventoryItem(
  draft: ProviderDraft,
  id: string,
  patch: { context_window_tokens?: number; supports_vision?: boolean }
): ProviderDraft {
  return {
    ...draft,
    models: draft.models.map((item) => {
      if (item.id !== id) return item
      const next: ProviderModelInventoryItem = { id }
      const contextWindow =
        'context_window_tokens' in patch ? patch.context_window_tokens : item.context_window_tokens
      if (contextWindow !== undefined) next.context_window_tokens = contextWindow
      const vision = 'supports_vision' in patch ? patch.supports_vision : item.supports_vision
      if (vision !== undefined) next.supports_vision = vision
      return next
    })
  }
}
