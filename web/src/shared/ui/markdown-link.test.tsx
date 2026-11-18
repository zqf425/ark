// @vitest-environment jsdom
/**
 * Prose link hand-off: a link in model prose must leave the app instead of
 * replacing the document. The bridge is mocked here; the scheme policy lives in
 * `shared/desktop.ts` and the Rust shell.
 */
import { act } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle } from '../testing/react-harness'
import { openExternal } from '../desktop'
import { Markdown } from './Markdown'

vi.mock('../desktop', () => ({ openExternal: vi.fn(async () => undefined) }))

installBrowserStubs()

async function dispatch(node: HTMLElement, type: string, init: MouseEventInit = {}): Promise<MouseEvent> {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, ...init })
  await act(async () => {
    node.dispatchEvent(event)
    await settle()
  })
  return event
}

describe('Markdown prose links', () => {
  beforeEach(() => {
    vi.mocked(openExternal).mockClear()
  })

  it('hands an external link to the shell instead of navigating', async () => {
    const h = await mountWithProviders(
      <Markdown text="见 [账号登录](https://accounts.google.com/signin)。" />
    )
    const anchor = h.host.querySelector('a')!
    expect(anchor.getAttribute('href')).toBe('https://accounts.google.com/signin')
    const event = await dispatch(anchor, 'click')
    expect(event.defaultPrevented).toBe(true)
    expect(openExternal).toHaveBeenCalledWith('https://accounts.google.com/signin')
    await h.unmount()
  })

  it('leaves in-page fragments to the document', async () => {
    const h = await mountWithProviders(<Markdown text="跳到 [本节](#section)。" />)
    const event = await dispatch(h.host.querySelector('a')!, 'click')
    expect(openExternal).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    await h.unmount()
  })

  it('hands a middle click over too, so no second window opens', async () => {
    const h = await mountWithProviders(<Markdown text="[文档](https://example.com/a)" />)
    const event = await dispatch(h.host.querySelector('a')!, 'auxclick', { button: 1 })
    expect(event.defaultPrevented).toBe(true)
    expect(openExternal).toHaveBeenCalledWith('https://example.com/a')
    await h.unmount()
  })
})
