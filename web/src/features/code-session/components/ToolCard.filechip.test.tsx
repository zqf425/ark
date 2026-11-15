// @vitest-environment jsdom
/**
 * Write-family tool cards expose a file chip (requirements/tool-file-preview.md):
 * clicking it opens the file in the inspector's File panel without toggling the
 * card's own fold state. Cards without a structured path render exactly as before.
 */
import { act } from 'react'
import { describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { ToolCard } from './ToolCard'

installBrowserStubs()

let seq = 0

function toolItem(tool: Partial<ThreadItem['tool']>): ThreadItem {
  seq += 1
  return {
    item_id: `00000000-0000-0000-0000-0000000000${seq.toString().padStart(2, '0')}`,
    turn_id: '00000000-0000-0000-0000-0000000000t1',
    kind: 'tool_call',
    status: 'completed',
    created_at: '2026-11-05T00:00:00Z',
    tool: {
      call_id: `c${seq}`,
      tool: 'write',
      arguments: { path: 'docs/a.md', content: 'x' },
      ok: true,
      ...tool
    }
  }
}

async function click(node: HTMLElement): Promise<void> {
  await act(async () => {
    node.click()
    await settle()
  })
}

describe('ToolCard file chip', () => {
  let harness: ReactHarness | null = null

  const mount = async (item: ThreadItem): Promise<ReactHarness> => {
    harness = await mountWithProviders(<ToolCard item={item} />)
    return harness
  }

  it('renders a clickable file chip on a write card', async () => {
    const h = await mount(toolItem({}))
    const chip = h.host.querySelector<HTMLButtonElement>('.ark-tool__filechip')
    expect(chip).not.toBeNull()
    expect(chip!.textContent).toContain('a.md')
    expect(chip!.textContent).toContain('docs')
    await h.unmount()
    harness = null
  })

  it('selectFile routes the chip to the inspector File panel and expands it', async () => {
    useCodeSessionStore.setState({ selectedFilePath: null, activePanel: 'changes', rightCollapsed: true })
    const h = await mount(toolItem({}))
    const chip = h.host.querySelector<HTMLButtonElement>('.ark-tool__filechip')!
    await click(chip)
    const state = useCodeSessionStore.getState()
    expect(state.selectedFilePath).toBe('docs/a.md')
    expect(state.activePanel).toBe('file')
    // A chip click is an explicit "open this file" intent: the inspector must
    // expand even though it starts collapsed (the collapsed state was the
    // reported "chip does nothing" bug).
    expect(state.rightCollapsed).toBe(false)
    await h.unmount()
    harness = null
  })

  it('clicking the chip does not toggle the card fold', async () => {
    const h = await mount(toolItem({}))
    const chip = h.host.querySelector<HTMLButtonElement>('.ark-tool__filechip')!
    await click(chip)
    const toggle = h.host.querySelector<HTMLButtonElement>('.ark-tool__headtoggle')!
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(h.host.querySelector('.ark-tool.is-open')).toBeNull()
    await h.unmount()
    harness = null
  })

  it('the fold toggle still works on its own button', async () => {
    const h = await mount(toolItem({}))
    const toggle = h.host.querySelector<HTMLButtonElement>('.ark-tool__headtoggle')!
    await click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(h.host.querySelector('.ark-tool.is-open')).not.toBeNull()
    await h.unmount()
    harness = null
  })

  it('omits the chip for tools without a path argument', async () => {
    const h = await mount(toolItem({ tool: 'bash', arguments: { cmd: 'echo hi' } }))
    expect(h.host.querySelector('.ark-tool__filechip')).toBeNull()
    await h.unmount()
    harness = null
  })

  it('omits the chip when the write payload has no path yet (streaming start)', async () => {
    const h = await mount(toolItem({ arguments: undefined }))
    expect(h.host.querySelector('.ark-tool__filechip')).toBeNull()
    await h.unmount()
    harness = null
  })
})
