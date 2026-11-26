// @vitest-environment jsdom
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, type ReactHarness } from '../../../shared/testing/react-harness'
import { ApiError } from '../../../shared/api/client'
import { useCodeSessionStore } from '../state/store'
import { startTurn } from '../api/turns-api'
import { useTurn } from './use-turn'

vi.mock('../api/turns-api', () => ({
  startTurn: vi.fn(),
  steerTurn: vi.fn(),
  interruptTurn: vi.fn()
}))

const THREAD_ID = '00000000-0000-0000-0000-00000000000a'

describe('useTurn failure classification', () => {
  let harness: ReactHarness | null = null
  let send: ((text: string) => Promise<unknown>) | null = null

  function Probe(): null {
    send = useTurn(THREAD_ID).send
    return null
  }

  beforeEach(() => {
    installBrowserStubs()
    useCodeSessionStore.setState({ sending: false, interrupting: false, streamStatus: 'open', activeTurnId: null })
    vi.mocked(startTurn).mockReset()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    send = null
  })

  async function deliverAfter(error: unknown): Promise<unknown> {
    vi.mocked(startTurn).mockRejectedValueOnce(error)
    harness = await mountWithProviders(<Probe />)
    let result: unknown
    await act(async () => {
      result = await send!('queued message')
    })
    return result
  }

  it.each([
    ['validation_error', 400, 'permanent'],
    ['not_found', 404, 'permanent'],
    ['unknown', 401, 'permanent'],
    ['conflict', 409, 'permanent']
  ] as const)('classifies %s HTTP %i as %s', async (code, status, expected) => {
    const result = await deliverAfter(new ApiError({ code, status, message: 'rejected' }))
    expect(result).toBe(expected)
  })

  it.each([
    ['unknown', 408, 'retry'],
    ['unknown', 425, 'retry'],
    ['runtime_unavailable', 503, 'retry'],
    ['unknown', 429, 'retry']
  ] as const)('classifies %s HTTP %i as %s', async (code, status, expected) => {
    const result = await deliverAfter(new ApiError({ code, status, message: 'try again' }))
    expect(result).toBe(expected)
  })

  it('never automatically resends a transport failure without an HTTP response', async () => {
    const result = await deliverAfter(new TypeError('network unavailable'))
    expect(result).toBe('permanent')
  })
})
