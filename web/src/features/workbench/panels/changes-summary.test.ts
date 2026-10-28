import { describe, expect, it } from 'vitest'
import type { WorkspaceChange } from '../../../shared/contracts'
import { changesSummary } from './changes-summary'

const turn = '00000000-0000-0000-0000-000000000001' as WorkspaceChange['turn_id']

describe('changesSummary', () => {
  it('reports files and the successful write operations that produced them', () => {
    const changes: WorkspaceChange[] = [
      { path: 'src/a.ts', operation: 'write', turn_id: turn, seq: 4 },
      { path: 'src/b.ts', operation: 'edit', turn_id: turn, seq: 3 }
    ]

    expect(changesSummary(changes)).toBe('2 个文件 · 1 个写入文件 · 1 个编辑文件')
  })

  it('does not label an unknown operation as a write or edit', () => {
    expect(changesSummary([{ path: 'src/a.ts', operation: 'rename', turn_id: turn, seq: 4 }])).toBe('1 个文件 · 1 项操作')
  })
})
