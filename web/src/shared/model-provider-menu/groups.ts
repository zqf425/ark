/**
 * Pure mapping from the redacted provider list to the two-level menu's groups.
 * Kept outside React so grouping and "which row is current" are unit-testable.
 */
import type { ProviderView } from '../contracts'

/** One atomic selection: a model id is only meaningful inside its provider. */
export type ModelProviderSelection = { providerId: string; model: string }

/** One left-column row: a provider and the models it can serve. */
export type ModelProviderGroup = {
  id: string
  label: string
  /** Default model first, then the imported inventory; trimmed and deduped. */
  models: string[]
}

function dedupeModels(ids: readonly string[]): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const raw of ids) {
    const id = raw.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    out.push(id)
  }
  return out
}

/** Build the left column; disabled providers stay visible (they can be re-enabled). */
export function toModelProviderGroups(providers: readonly ProviderView[]): ModelProviderGroup[] {
  return providers.map((provider) => ({
    id: provider.id,
    label: provider.label?.trim() || provider.id,
    models: dedupeModels([provider.model, ...provider.models.map((item) => item.id)])
  }))
}

export type ResolvedSelection = {
  group: ModelProviderGroup | null
  model: string
  /** True when the value came from `fallback`, not an explicit override. */
  isDefault: boolean
}

/**
 * Groups for the "default model" setting. The runtime stores a provider id
 * plus an optional pinned model, so each provider offers its stored `model`
 * and the whole imported inventory: picking any row pins that exact
 * `(provider, model)` pair as the default.
 */
export function toDefaultModelGroups(providers: readonly ProviderView[]): ModelProviderGroup[] {
  return toModelProviderGroups(providers)
}

/**
 * An explicit `(providerId, model)` always wins; a blank provider id previews
 * the effective runtime default (active provider) without pinning an override.
 */
export function resolveModelProviderSelection(
  groups: readonly ModelProviderGroup[],
  value: ModelProviderSelection,
  fallback?: ModelProviderSelection
): ResolvedSelection {
  const providerId = value.providerId.trim()
  const model = value.model.trim()
  if (providerId) {
    return { group: groups.find((group) => group.id === providerId) ?? null, model, isDefault: false }
  }
  const hintId = fallback?.providerId.trim() ?? ''
  return {
    group: hintId ? groups.find((group) => group.id === hintId) ?? null : null,
    model: fallback?.model.trim() ?? '',
    isDefault: true
  }
}
