// @vitest-environment jsdom
/**
 * CopyButton contract: renders a quiet icon button, calls the clipboard with
 * the exact payload, flips to the copied state for the feedback window and
 * reverts. Failure must not flip the state.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { installBrowserStubs, mountWithProviders } from '../testing/react-harness'
import { COPIED_FEEDBACK_MS, CopyButton, copyText } from './copy'

installBrowserStubs()

type ClipboardStub = { writeText: (text: string) => Promise<void> }

function stubClipboard(impl?: (text: string) => Promise<void> | void): ClipboardStub {
  const stub: ClipboardStub = {
    writeText: (text: string) => Promise.resolve(impl?.(text))
  }
  Object.defineProperty(navigator, 'clipboard', { value: stub, configurable: true, writable: true })
  return stub
}

afterEach(() => {
  // jsdom has no clipboard; drop the stub so tests stay independent.
  Reflect.deleteProperty(navigator, 'clipboard')
  vi.restoreAllMocks()
})

describe('copyText', () => {
  it('优先走异步 Clipboard API 并返回 true', async () => {
    const written: string[] = []
    stubClipboard((text) => {
      written.push(text)
    })
    await expect(copyText('hello')).resolves.toBe(true)
    expect(written).toEqual(['hello'])
  })

  it('Clipboard API 缺失时回落 execCommand', async () => {
    stubClipboard()
    Reflect.deleteProperty(navigator, 'clipboard')
    // jsdom does not implement execCommand at all — install the minimal stub.
    const executed = vi.fn<() => boolean>().mockReturnValue(true)
    Object.defineProperty(document, 'execCommand', { value: executed, configurable: true, writable: true })
    await expect(copyText('legacy')).resolves.toBe(true)
    expect(executed).toHaveBeenCalledWith('copy')
    Reflect.deleteProperty(document, 'execCommand')
  })

  it('复制被拒绝时返回 false 而不是抛错', async () => {
    stubClipboard(() => {
      throw new Error('denied')
    })
    await expect(copyText('nope')).resolves.toBe(false)
  })
})

describe('CopyButton', () => {
  it('点击后写入剪贴板并进入 copied 状态,超时后复原', async () => {
    stubClipboard()
    vi.useFakeTimers()
    const harness = await mountWithProviders(
      <CopyButton text="payload" label="复制" copiedLabel="已复制" />
    )
    const button = harness.host.querySelector('button.ark-copy')
    expect(button).not.toBeNull()
    expect(button?.getAttribute('aria-label')).toBe('复制')

    await act(async () => {
      button!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(navigator.clipboard.writeText).toBeTruthy()
    expect(button?.className).toContain('is-copied')
    expect(button?.getAttribute('aria-label')).toBe('已复制')

    await act(async () => {
      vi.advanceTimersByTime(COPIED_FEEDBACK_MS + 10)
    })
    expect(button?.className).not.toContain('is-copied')
    vi.useRealTimers()
    await harness.unmount()
  })

  it('复制失败不进入 copied 状态', async () => {
    stubClipboard(() => {
      throw new Error('denied')
    })
    const harness = await mountWithProviders(
      <CopyButton text="payload" label="复制" copiedLabel="已复制" />
    )
    const button = harness.host.querySelector('button.ark-copy')!
    await act(async () => {
      button.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(button.className).not.toContain('is-copied')
    await harness.unmount()
    Reflect.deleteProperty(document, 'execCommand')
  })
})
