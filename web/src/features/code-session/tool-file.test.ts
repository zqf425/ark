import { describe, expect, it } from 'vitest'
import type { ToolPayload } from '../../shared/contracts'
import { toolTargetPath } from './tool-file'

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
