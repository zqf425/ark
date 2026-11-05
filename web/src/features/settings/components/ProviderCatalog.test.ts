import { describe, expect, it } from 'vitest'
import type { ProviderModelSummary } from '../../../shared/contracts'
import {
  buildImportCandidates,
  defaultImportSelection,
  mergeImportedModels,
  parsePastedModelIds,
  updateInventoryItem
} from './ProviderCatalog'
import { emptyDraft } from './ProviderDraft'

const catalogRow = (id: string): ProviderModelSummary => ({ id })

describe('provider batch model import helpers', () => {
  it('parses pasted ids one per line without empties or duplicates', () => {
    expect(parsePastedModelIds(' a\n\nb \na\n')).toEqual(['a', 'b'])
    expect(parsePastedModelIds('')).toEqual([])
  })

  it('marks rows already present in the provider as such', () => {
    const draft = { ...emptyDraft(), model: 'default', models: [{ id: 'inv-1' }] }
    const rows = buildImportCandidates([catalogRow('default'), catalogRow('inv-1'), catalogRow('fresh')], 'pasted-1\n', draft)
    expect(rows.map((row) => [row.id, row.alreadyPresent])).toEqual([
      ['default', true],
      ['inv-1', true],
      ['fresh', false],
      ['pasted-1', false]
    ])
  })

  it('carries catalog metadata onto candidate rows', () => {
    const draft = { ...emptyDraft(), model: 'default' }
    const rows = buildImportCandidates(
      [{ id: 'vision', context_window_tokens: 128_000, supports_vision: true }, catalogRow('plain')],
      '',
      draft
    )
    expect(rows).toEqual([
      {
        id: 'vision',
        label: 'vision — 128000 tokens · 支持识图',
        alreadyPresent: false,
        context_window_tokens: 128_000,
        supports_vision: true
      },
      { id: 'plain', label: 'plain', alreadyPresent: false }
    ])
  })

  it('defaults the selection to everything not already present', () => {
    const draft = { ...emptyDraft(), model: 'default' }
    const rows = buildImportCandidates([catalogRow('default'), catalogRow('a'), catalogRow('b')], '', draft)
    expect([...defaultImportSelection(rows)]).toEqual(['a', 'b'])
  })

  // A catalog can list hundreds of ids while the store accepts at most
  // PROVIDER_MODELS_MAX. Selecting all of them would open the dialog with a
  // permanently disabled confirm button.
  it('caps the default selection at the inventory ceiling', () => {
    const draft = { ...emptyDraft(), model: 'default' }
    const many = Array.from({ length: 100 }, (_, index) => catalogRow(`model-${index}`))
    const rows = buildImportCandidates(many, '', draft)

    const selection = defaultImportSelection(rows)

    expect(selection.size).toBe(64)
    expect([...selection][0]).toBe('model-0')
    expect([...selection].at(-1)).toBe('model-63')
  })

  it('merges the selection into the inventory without touching the default', () => {
    const draft = { ...emptyDraft(), model: 'default', models: [{ id: 'inv-1' }] }
    const merged = mergeImportedModels(draft, [
      { id: 'default', label: null, alreadyPresent: true },
      { id: 'inv-1', label: null, alreadyPresent: true },
      { id: 'new', label: null, alreadyPresent: false }
    ])
    expect(merged.model).toBe('default')
    // Existing inventory rows survive; the merge only appends fresh ids.
    expect(merged.models).toEqual([{ id: 'inv-1' }, { id: 'new' }])
  })

  it('mirrors the default model\'s catalog metadata onto the entry-level fields', () => {
    const draft = { ...emptyDraft(), model: 'vision-model' }
    const merged = mergeImportedModels(draft, [
      { id: 'vision-model', label: null, alreadyPresent: false, context_window_tokens: 128_000, supports_vision: true }
    ])
    expect(merged.model).toBe('vision-model')
    // The store strips `model` from the inventory, so the default model's
    // metadata must live at entry level for the runtime to read it.
    expect(merged.models).toEqual([])
    expect(merged.context_window_tokens).toBe('128000')
    expect(merged.supports_vision).toBe(true)
  })

  it('backfills the default model from the selection when the form has none', () => {
    const draft = { ...emptyDraft(), model: '' }
    const merged = mergeImportedModels(draft, [
      { id: 'first', label: null, alreadyPresent: false },
      { id: 'second', label: null, alreadyPresent: false }
    ])
    expect(merged.model).toBe('first')
    expect(merged.models).toEqual([{ id: 'second' }])
  })

  it('keeps catalog metadata across the merge', () => {
    const draft = { ...emptyDraft(), model: 'default' }
    const merged = mergeImportedModels(draft, [
      { id: 'vision', label: null, alreadyPresent: false, context_window_tokens: 128_000, supports_vision: true }
    ])
    expect(merged.models).toEqual([{ id: 'vision', context_window_tokens: 128_000, supports_vision: true }])
  })

  it('patches one inventory row in place without touching the others', () => {
    const draft = { ...emptyDraft(), model: 'default', models: [{ id: 'a' }, { id: 'b' }] }

    const patched = updateInventoryItem(draft, 'b', { supports_vision: true, context_window_tokens: 32_000 })
    expect(patched.models).toEqual([
      { id: 'a' },
      { id: 'b', context_window_tokens: 32_000, supports_vision: true }
    ])

    // Clearing restores "unknown": the key is dropped, not written as null.
    const cleared = updateInventoryItem(patched, 'b', { supports_vision: undefined })
    expect(cleared.models[1]).toEqual({ id: 'b', context_window_tokens: 32_000 })
  })
})
