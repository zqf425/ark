// @vitest-environment jsdom
import { act } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { startTurn } from '../api/turns-api'
import { Composer } from './Composer'

vi.mock('../api/turns-api', () => ({
  startTurn: vi.fn(), steerTurn: vi.fn(), interruptTurn: vi.fn()
}))
vi.mock('./ComposerRuntimeControls', () => ({
  AutonomyPicker: () => null, WorkspacePicker: () => null, ContextRing: () => null, ModelPicker: () => null
}))
vi.mock('../api/skills-api', () => ({
  listSkills: vi.fn(async () => ({ skills: [] })),
  listThreadSkills: vi.fn(async () => ({
    enabled: true, base_roots: [], extra_roots: [], loaded_extra_roots: [],
    base_skills: [], extra_skills: [], warnings: [], base_validation_errors: []
  })),
  refreshThreadSkills: vi.fn()
}))

const THREAD_ID = '00000000-0000-0000-0000-00000000000a'

describe('Composer turn delivery recovery', () => {
  let harness: ReactHarness | null = null

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('does not resend an accepted turn when the HTTP response is lost', async () => {
    vi.useFakeTimers()
    installBrowserStubs()
    useCodeSessionStore.setState({
      drafts: {}, queuedMessages: {}, sending: false, streamStatus: 'open', activeTurnId: null
    })
    useCodeSessionStore.getState().queueMessage(THREAD_ID, 'only deliver once')
    vi.mocked(startTurn).mockRejectedValue(new TypeError('response connection lost'))
    harness = await mountWithProviders(<Composer threadId={THREAD_ID} />)
    await settle()
    expect(startTurn).toHaveBeenCalledTimes(1)
    await act(async () => {
      vi.advanceTimersByTime(5_000)
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(startTurn).toHaveBeenCalledTimes(1)
    expect(useCodeSessionStore.getState().queuedMessages[THREAD_ID]).toHaveLength(1)
  })
})
