import { describe, expect, it } from 'vitest'
import { isLatestWorkspaceProbe, shouldProbeWorkspace } from './workspace-probe-guard'

describe('isLatestWorkspaceProbe', () => {
  it('rejects a response from a probe superseded by a newer request', () => {
    expect(isLatestWorkspaceProbe(2, 1)).toBe(false)
  })

  it('accepts the response belonging to the active request', () => {
    expect(isLatestWorkspaceProbe(2, 2)).toBe(true)
  })

  it('does not probe a workspace when no thread is selected', () => {
    expect(shouldProbeWorkspace(null, 'D:/code/ark')).toBe(false)
    expect(shouldProbeWorkspace('thread-1', '')).toBe(false)
    expect(shouldProbeWorkspace('thread-1', 'D:/code/ark')).toBe(true)
  })
})
