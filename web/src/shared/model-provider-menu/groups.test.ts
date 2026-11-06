import { describe, expect, it } from 'vitest'
import type { ProviderView } from '../contracts'
import { resolveModelProviderSelection, toDefaultModelGroups, toModelProviderGroups } from './groups'

function provider(id: string, model: string, inventory: string[] = []): ProviderView {
  return {
    id,
    base_url: 'https://example.test',
    endpoint_format: 'openai_chat',
    model,
    enabled: true,
    is_default: false,
    has_api_key: true,
    models: inventory.map((item) => ({ id: item }))
  }
}

describe('two-level provider/model menu grouping', () => {
  it('puts the default model first, trims ids and drops duplicates', () => {
    const groups = toModelProviderGroups([
      provider('p1', 'glm', ['glm', ' qwen ', '', 'kimi']),
      provider('p2', 'gpt', [])
    ])
    expect(groups).toEqual([
      { id: 'p1', label: 'p1', models: ['glm', 'qwen', 'kimi'] },
      { id: 'p2', label: 'p2', models: ['gpt'] }
    ])
  })

  it('uses the provider label when present', () => {
    const withLabel = { ...provider('p', 'm'), label: 'DeepSeek 官方' }
    const [first] = toModelProviderGroups([withLabel])
    expect(first?.label).toBe('DeepSeek 官方')
  })

  it('keeps disabled providers and empty inventories visible', () => {
    const disabled = { ...provider('off', ''), enabled: false }
    expect(toModelProviderGroups([disabled])).toEqual([{ id: 'off', label: 'off', models: [] }])
  })

  it('default-provider groups expose exactly the configured default model', () => {
    const groups = toDefaultModelGroups([
      provider('p1', 'glm', ['glm', 'qwen', 'kimi']),
      provider('p2', 'gpt', [])
    ])
    expect(groups).toEqual([
      { id: 'p1', label: 'p1', models: ['glm'] },
      { id: 'p2', label: 'p2', models: ['gpt'] }
    ])
  })

  it('resolves an explicit selection over the fallback', () => {
    const groups = toModelProviderGroups([provider('p1', 'glm', ['qwen']), provider('p2', 'gpt')])
    const resolved = resolveModelProviderSelection(
      groups, { providerId: 'p2', model: 'gpt' }, { providerId: 'p1', model: 'glm' }
    )
    expect(resolved).toEqual({ group: groups[1], model: 'gpt', isDefault: false })
  })

  it('previews the effective default when no override is stored', () => {
    const groups = toModelProviderGroups([provider('p1', 'glm'), provider('p2', 'gpt')])
    const resolved = resolveModelProviderSelection(
      groups, { providerId: '', model: '' }, { providerId: 'p1', model: 'glm' }
    )
    expect(resolved).toEqual({ group: groups[0], model: 'glm', isDefault: true })
  })

  it('reports no group when the named provider is gone and there is no fallback', () => {
    const groups = toModelProviderGroups([provider('p1', 'glm')])
    const resolved = resolveModelProviderSelection(groups, { providerId: 'ghost', model: 'x' })
    expect(resolved.group).toBeNull()
    expect(resolved.model).toBe('x')
  })
})
