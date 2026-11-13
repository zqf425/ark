/**
 * Session-level workspace override rules.
 *
 * The composer workspace pick is a per-session setting (matching mainstream
 * coding agents): it must write the thread override and never touch the
 * host-global root, and it stays editable unless the model is currently
 * streaming in THIS session. Other sessions running turns must not block it.
 * Kept as pure predicates so the regression does not need a DOM.
 */
import { describe, expect, it } from 'vitest'

/**
 * Mirrors the decision encoded in `WorkspacePicker` so a future refactor of
 * the component cannot silently re-introduce the global switch call or a
 * process-wide busy gate.
 */
export type WorkspacePickDecision = {
  threadId: string | null
  activeTurnId: string | null
  loadedFor: string | null
}

export function canChangeSessionWorkspace(decision: WorkspacePickDecision): boolean {
  if (decision.threadId === null || decision.activeTurnId !== null) return false
  return decision.loadedFor !== null && decision.loadedFor === decision.threadId
}

describe('canChangeSessionWorkspace', () => {
  it('allows changing the workspace while OTHER sessions run turns', () => {
    // The host-global busy state is irrelevant: only this session's turn
    // matters, so the decision input has no process-wide activity field.
    expect(canChangeSessionWorkspace({ threadId: 't1', activeTurnId: null, loadedFor: 't1' })).toBe(true)
  })

  it('blocks only while THIS session is streaming a reply', () => {
    expect(canChangeSessionWorkspace({ threadId: 't1', activeTurnId: 'turn-9', loadedFor: 't1' })).toBe(false)
  })

  it('requires the override form to be loaded for the same thread', () => {
    expect(canChangeSessionWorkspace({ threadId: 't1', activeTurnId: null, loadedFor: null })).toBe(false)
    expect(canChangeSessionWorkspace({ threadId: 't1', activeTurnId: null, loadedFor: 't2' })).toBe(false)
  })

  it('never enables an unselected session', () => {
    expect(canChangeSessionWorkspace({ threadId: null, activeTurnId: null, loadedFor: 't1' })).toBe(false)
  })
})
