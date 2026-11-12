// @vitest-environment jsdom
/**
 * The shared TextField's secret-reveal contract.
 *
 * Pinned here: a revealable field masks the draft by default, toggles to
 * plaintext and back without dropping the value, and announces its state via
 * aria-pressed. Plain text fields are unchanged. Only the value the operator
 * typed is ever revealed — stored keys are write-only and never reach here.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { installBrowserStubs, mountWithProviders, type ReactHarness } from '../../../shared/testing/react-harness'
import { TextField } from './Field'

describe('TextField (revealable secret)', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => installBrowserStubs())
  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  const input = (): HTMLInputElement => {
    const node = harness?.host.querySelector('#secret')
    if (!node) throw new Error('secret input missing')
    return node as HTMLInputElement
  }

  const toggle = (): HTMLButtonElement => {
    const node = harness?.host.querySelector('.ark-field__secret-toggle')
    if (!node) throw new Error('reveal toggle missing')
    return node as HTMLButtonElement
  }

  it('masks by default and toggles to plaintext and back without losing the value', async () => {
    harness = await mountWithProviders(
      <TextField id="secret" label="api_key" value="sk-test-123" revealable onChange={() => undefined} />
    )
    expect(input().type).toBe('password')
    expect(input().value).toBe('sk-test-123')
    expect(toggle().getAttribute('aria-pressed')).toBe('false')

    await act(async () => { toggle().click() })
    expect(input().type).toBe('text')
    expect(input().value).toBe('sk-test-123')
    expect(toggle().getAttribute('aria-pressed')).toBe('true')

    await act(async () => { toggle().click() })
    expect(input().type).toBe('password')
    expect(input().value).toBe('sk-test-123')
    expect(toggle().getAttribute('aria-pressed')).toBe('false')
  })

  it('leaves ordinary text fields alone: no toggle, declared type kept', async () => {
    harness = await mountWithProviders(
      <TextField id="plain" label="id" value="gateway" onChange={() => undefined} />
    )
    expect(harness.host.querySelector('.ark-field__secret-toggle')).toBeNull()
    expect((harness.host.querySelector('#plain') as HTMLInputElement).type).toBe('text')
  })
})
