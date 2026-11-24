// @vitest-environment jsdom
/**
 * Timeline tool rows (Qoder-referenced session layout): every tool row opens
 * with a lifecycle status node on the process card's rail — green check when
 * the call succeeded, red cross when it failed, spinning arc while it runs —
 * and carries a localized tool-family label so a long turn scans by meaning
 * ("终端命令", "读取文件") before the raw mono tool name.
 */
import { afterEach, describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { ToolCard } from './ToolCard'

installBrowserStubs()

let seq = 0

function toolItem(tool: Partial<NonNullable<ThreadItem['tool']>>, status: ThreadItem['status'] = 'completed'): ThreadItem {
  seq += 1
  return {
    item_id: `00000000-0000-0000-0000-0000000001${seq.toString().padStart(2, '0')}`,
    turn_id: '00000000-0000-0000-0000-0000000000t1',
    kind: 'tool_call',
    status,
    created_at: '2026-11-20T00:00:00Z',
    tool: {
      call_id: `c${seq}`,
      tool: 'bash',
      arguments: { cmd: 'echo hi' },
      ...tool
    }
  }
}

describe('ToolCard timeline row', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('shows a success status node for a succeeded call', async () => {
    const h = await mountWithProviders(<ToolCard item={toolItem({ ok: true })} />)
    expect(h.host.querySelector('.ark-tool__status--ok')).not.toBeNull()
    expect(h.host.querySelector('.ark-tool__status--fail')).toBeNull()
    expect(h.host.querySelector('.ark-tool__status--live')).toBeNull()
  })

  it('shows a failure status node for a failed call', async () => {
    const h = await mountWithProviders(<ToolCard item={toolItem({ ok: false })} />)
    expect(h.host.querySelector('.ark-tool__status--fail')).not.toBeNull()
    expect(h.host.querySelector('.ark-tool__status--ok')).toBeNull()
  })

  it('shows a spinning status node while the call runs', async () => {
    const h = await mountWithProviders(<ToolCard item={toolItem({}, 'streaming')} />)
    expect(h.host.querySelector('.ark-tool__status--live')).not.toBeNull()
    expect(h.host.querySelector('.ark-tool__status--ok')).toBeNull()
  })

  it('labels the exec family in the active locale', async () => {
    const h = await mountWithProviders(<ToolCard item={toolItem({ ok: true })} />)
    const family = h.host.querySelector('.ark-tool__family')
    expect(family).not.toBeNull()
    expect(family!.textContent).toBe('终端命令')
    // The raw tool name stays visible for precision.
    expect(h.host.querySelector('.ark-tool__name')!.textContent).toBe('bash')
  })

  it('labels read / write / search families', async () => {
    const read = await mountWithProviders(<ToolCard item={toolItem({ tool: 'read', arguments: { path: 'a.ts' }, ok: true })} />)
    expect(read.host.querySelector('.ark-tool__family')!.textContent).toBe('读取文件')
    await read.unmount()
    const write = await mountWithProviders(<ToolCard item={toolItem({ tool: 'edit', arguments: { path: 'a.ts' }, ok: true })} />)
    expect(write.host.querySelector('.ark-tool__family')!.textContent).toBe('编辑文件')
    await write.unmount()
    const search = await mountWithProviders(<ToolCard item={toolItem({ tool: 'grep', arguments: { pattern: 'x' }, ok: true })} />)
    expect(search.host.querySelector('.ark-tool__family')!.textContent).toBe('搜索内容')
  })
})