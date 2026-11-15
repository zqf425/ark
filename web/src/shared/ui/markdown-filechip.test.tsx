// @vitest-environment jsdom
/**
 * Prose file-chip wiring (requirements/prose-path-links.md): the shared
 * Markdown's delegated click must route a promoted chip to the onFileChip
 * callback, and only files on the turn's whitelist may reach the store.
 */
import { act } from 'react'
import { describe, expect, it } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle } from '../testing/react-harness'
import { Markdown } from './Markdown'

installBrowserStubs()

async function click(node: HTMLElement): Promise<void> {
  await act(async () => {
    node.click()
    await settle()
  })
}

describe('Markdown prose file chip', () => {
  it('delegates a data-md-file click with the chip path', async () => {
    const seen: string[] = []
    const h = await mountWithProviders(
      <Markdown text="写入 `docs/a.md` 完成。" onFileChip={(p) => seen.push(p)} writablePaths={['docs/a.md']} />
    )
    console.log('HOST-HTML:', h.host.innerHTML.slice(0, 200))
    const chip = h.host.querySelector<HTMLButtonElement>('.ark-md-filechip')!
    expect(chip).toBeTruthy()
    await click(chip)
    expect(seen).toEqual(['docs/a.md'])
    await h.unmount()
  })

  it('absent callback: chip never renders', async () => {
    const h = await mountWithProviders(<Markdown text="写入 `docs/a.md` 完成。" />)
    expect(h.host.querySelector('.ark-md-filechip')).toBeNull()
    await h.unmount()
  })
})
