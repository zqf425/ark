import { describe, expect, it } from 'vitest'
import type { ProviderModelSummary } from '../../../shared/contracts'
import {
  buildImportCandidates,
  defaultImportSelection,
  mergeImportedModels,
  parsePastedModelIds
} from './ProviderCatalog'
import { emptyDraft } from './ProviderDraft'

const catalogRow = (id: string): ProviderModelSummary => ({ id })

describe('provider batch model import helpers', () => {
  it('parses pasted ids one per line without empties or duplicates', () => {
    expect(parsePastedModelIds(' a\n\nb \na\n')).toEqual(['a', 'b'])
    expect(parsePastedModelIds('')).toEqual([])
  })

  it('marks rows already present in the provider as such', () => {
    const draft = { ...emptyDraft(), model: 'default', models: ['inv-1'] }
    const rows = buildImportCandidates([catalogRow('default'), catalogRow('inv-1'), catalogRow('fresh')], 'pasted-1\n', draft)
    expect(rows.map((row) => [row.id, row.alreadyPresent])).toEqual([
      ['default', true],
      ['inv-1', true],
      ['fresh', false],
      ['pasted-1', false]
    ])
  })

  it('defaults the selection to everything not already present', () => {
    const draft = { ...emptyDraft(), model: 'default' }
    const rows = buildImportCandidates([catalogRow('default'), catalogRow('a'), catalogRow('b')], '', draft)
    expect([...defaultImportSelection(rows)]).toEqual(['a', 'b'])
  })

  it('merges the selection into the inventory without touching the default', () => {
    const draft = { ...emptyDraft(), model: 'default', models: ['inv-1'] }
    const merged = mergeImportedModels(draft, ['default', 'inv-1', 'new'])
    expect(merged.model).toBe('default')
    expect(merged.models).toEqual(['inv-1', 'new'])
  })

  it('backfills the default model from the selection when the form has none', () => {
    const draft = { ...emptyDraft(), model: '' }
    const merged = mergeImportedModels(draft, ['first', 'second'])
    expect(merged.model).toBe('first')
    expect(merged.models).toEqual(['second'])
  })

  it('deduplicates repeated merges', () => {
    const draft = { ...emptyDraft(), model: 'default' }
    const once = mergeImportedModels(draft, ['a', 'b'])
    expect(mergeImportedModels(once, ['a', 'c']).models).toEqual(['a', 'b', 'c'])
  })
})
