import { describe, expect, it } from 'vitest'
import { workspaceBasename, workspaceOptions } from './workspace-options'

function descriptor(path: string) {
  return { path, generation: 1 }
}

describe('workspaceOptions', () => {
  it('puts the thread override first and dedupes the active root', () => {
    const result = workspaceOptions({
      override: '/code/ark',
      active: descriptor('/code/ark'),
      recent: [descriptor('/code/qdc'), descriptor('/code/smart-ark')]
    })
    expect(result.current).toBe('/code/ark')
    expect(result.options.map((option) => option.path)).toEqual([
      '/code/ark',
      '/code/qdc',
      '/code/smart-ark'
    ])
  })

  it('falls back to the host active root when no override is stored', () => {
    const result = workspaceOptions({ active: descriptor('/code/ark'), recent: [] })
    expect(result.current).toBe('/code/ark')
    expect(result.options).toHaveLength(1)
  })

  it('keeps a stale override visible even when it is not in recents', () => {
    const result = workspaceOptions({
      override: '/elsewhere/legacy',
      active: descriptor('/code/ark'),
      recent: []
    })
    expect(result.options[0]?.path).toBe('/elsewhere/legacy')
    expect(result.options).toHaveLength(2)
  })

  it('shows the parent folder only when it differs from the label', () => {
    const [option] = workspaceOptions({ recent: [descriptor('/code/ark')] }).options
    expect(option?.label).toBe('ark')
    expect(option?.parent).toBe('code')
    expect(workspaceBasename('/code/ark/')).toBe('ark')
    expect(workspaceBasename('C:\\work\\ark')).toBe('ark')
  })
})
