import { beforeEach, describe, expect, it } from 'vitest'
import { useCodeSessionStore } from './store'

/**
 * `onError` writes the transport diagnostic, then the SSE loop emits a bare
 * `onStatus('reconnecting')` immediately after. A default `error = null` on the
 * setter used to erase that diagnostic before the footer could render it.
 */
describe('setStreamStatus error preservation', () => {
  beforeEach(() => {
    useCodeSessionStore.getState().resetItems()
    useCodeSessionStore.setState({ streamError: null })
  })

  it('keeps the error across a bare reconnecting status update', () => {
    const { setStreamStatus } = useCodeSessionStore.getState()
    setStreamStatus('reconnecting', 'network unreachable')
    expect(useCodeSessionStore.getState().streamError).toBe('network unreachable')

    setStreamStatus('reconnecting')
    expect(useCodeSessionStore.getState().streamStatus).toBe('reconnecting')
    expect(useCodeSessionStore.getState().streamError).toBe('network unreachable')
  })

  it('clears the error when the stream recovers to open', () => {
    const { setStreamStatus } = useCodeSessionStore.getState()
    setStreamStatus('reconnecting', 'network unreachable')
    setStreamStatus('open')
    expect(useCodeSessionStore.getState().streamError).toBeNull()
  })

  it('clears the error when an explicit null is passed', () => {
    const { setStreamStatus } = useCodeSessionStore.getState()
    setStreamStatus('reconnecting', 'network unreachable')
    setStreamStatus('reconnecting', null)
    expect(useCodeSessionStore.getState().streamError).toBeNull()
  })
})
