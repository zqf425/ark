import { describe, expect, it } from 'vitest'
import type { ToolPayload } from '../../shared/contracts'
import { threadWritePaths, toolTargetPath } from './tool-file'

function payload(overrides: Partial<ToolPayload>): ToolPayload {
  return { call_id: 'c1', tool: 'write', arguments: { path: 'docs/a.md', content: 'x' }, ...overrides }
}

describe('toolTargetPath', () => {
  it('extracts path/name/dir from a write tool', () => {
    expect(toolTargetPath(payload({}))).toEqual({ path: 'docs/a.md', name: 'a.md', dir: 'docs' })
  })

  it('handles root-level files and windows separators in the label only', () => {
    expect(toolTargetPath(payload({ arguments: { path: 'README.md' } }))).toEqual({
      path: 'README.md',
      name: 'README.md',
      dir: ''
    })
    const win = toolTargetPath(payload({ arguments: { path: 'web\\src\\App.tsx' } }))
    expect(win).toEqual({ path: 'web\\src\\App.tsx', name: 'App.tsx', dir: 'web/src' })
  })

  it('works for the edit tool', () => {
    expect(toolTargetPath(payload({ tool: 'edit', arguments: { path: 'crates/x/src/lib.rs' } }))?.name).toBe('lib.rs')
  })

  it('returns null for tools without a structured path argument', () => {
    expect(toolTargetPath(payload({ tool: 'bash', arguments: { cmd: 'echo hi > a.md' } }))).toBeNull()
    expect(toolTargetPath(payload({ tool: 'read', arguments: { path: 'docs/a.md' } }))).toBeNull()
  })

  it('returns null for malformed arguments', () => {
    expect(toolTargetPath(payload({ arguments: undefined }))).toBeNull()
    expect(toolTargetPath(payload({ arguments: 'docs/a.md' } as unknown as ToolPayload))).toBeNull()
    expect(toolTargetPath(payload({ arguments: { path: 42 } } as unknown as ToolPayload))).toBeNull()
    expect(toolTargetPath(payload({ arguments: { path: '   ' } }))).toBeNull()
    expect(toolTargetPath(payload({ arguments: ['docs/a.md'] } as unknown as ToolPayload))).toBeNull()
  })
})

describe('threadWritePaths', () => {
  it('按首次出现顺序去重收集 write/edit 路径', () => {
    const items = [
      { tool: { call_id: 'c1', tool: 'write', arguments: { path: 'a.md', content: '' } } },
      { tool: { call_id: 'c2', tool: 'bash', arguments: { cmd: 'x' } } },
      { tool: { call_id: 'c3', tool: 'edit', arguments: { path: 'b/c.ts' } } },
      { tool: { call_id: 'c4', tool: 'write', arguments: { path: 'a.md', content: '' } } }
    ]
    expect(threadWritePaths(items)).toEqual(['a.md', 'b/c.ts'])
  })

  it('无工具或无 path 时返回空数组', () => {
    expect(threadWritePaths([{ tool: undefined }])).toEqual([])
    expect(threadWritePaths([])).toEqual([])
  })
})
