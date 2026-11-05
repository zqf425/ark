import { describe, expect, it } from 'vitest'
import { buildEntry, emptyDraft } from './ProviderDraft'
import { mergeImportedModels, updateInventoryItem } from './ProviderCatalog'

describe('provider model inventory helpers', () => {
  it('imports catalog metadata into the entry the user saves', () => {
    const draft = {
      ...emptyDraft(),
      id: 'gateway',
      base_url: 'https://gateway.example/v1',
      api_key: 'secret',
      model: 'vision-model'
    }

    const merged = mergeImportedModels(draft, [
      { id: 'vision-model', label: null, alreadyPresent: false, context_window_tokens: 128_000, supports_vision: true },
      { id: 'plain-model', label: null, alreadyPresent: false }
    ])
    const built = buildEntry(merged, false)

    expect(built).toMatchObject({
      ok: true,
      entry: {
        model: 'vision-model',
        context_window_tokens: 128_000,
        supports_vision: true,
        // A row with no catalog metadata is persisted with the 256K fallback,
        // so the runtime never sees an unknown window after an import.
        models: [{ id: 'plain-model', context_window_tokens: 256_000, supports_vision: undefined }]
      }
    })
  })

  it('keeps per-model metadata valid: a non-positive window fails the save', () => {
    const draft = {
      ...emptyDraft(),
      id: 'gateway',
      base_url: 'https://gateway.example/v1',
      api_key: 'secret',
      model: 'default',
      models: [{ id: 'bad', context_window_tokens: 0 }]
    }

    const built = buildEntry(draft, false)

    expect(built).toMatchObject({ ok: false })
  })

  it('patches one inventory row in place without touching the others', () => {
    const draft = {
      ...emptyDraft(),
      model: 'default',
      models: [
        { id: 'a', context_window_tokens: 128_000 },
        { id: 'b' }
      ]
    }

    const patched = updateInventoryItem(draft, 'b', { supports_vision: true, context_window_tokens: 32_000 })

    expect(patched.models).toEqual([
      { id: 'a', context_window_tokens: 128_000 },
      { id: 'b', context_window_tokens: 32_000, supports_vision: true }
    ])

    // Clearing restores "unknown": the key is dropped, not written as null.
    const cleared = updateInventoryItem(patched, 'a', { context_window_tokens: undefined })
    expect(cleared.models[0]).toEqual({ id: 'a' })
  })
})
