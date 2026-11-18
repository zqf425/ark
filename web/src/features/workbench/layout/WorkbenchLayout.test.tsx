// @vitest-environment jsdom
/**
 * Workbench 三列布局的交互接线：
 * - 检查器（右列）左缘的 resizer 向左拖变宽（右锚定），键盘同样以"变宽"为直觉方向；
 * - 左列（会话列表）展开态头部有收起入口，收起后经边轨按钮恢复。
 */
import { act } from 'react'
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../../code-session/state/store'
import { WorkbenchLayout } from './WorkbenchLayout'

describe('WorkbenchLayout 布局交互', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    useCodeSessionStore.setState({ leftWidth: 272, rightWidth: 392, leftCollapsed: false, rightCollapsed: false })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  afterAll(() => {
    const proto = HTMLElement.prototype as unknown as Record<string, unknown>
    delete proto.setPointerCapture
    delete proto.releasePointerCapture
  })

  // jsdom 无 pointer capture；与 use-panel-resizer.test 的 stub 同源，只装一次。
  beforeAll(() => {
    const proto = HTMLElement.prototype as unknown as Record<string, unknown>
    proto.setPointerCapture = vi.fn()
    proto.releasePointerCapture = vi.fn()
  })

  function firePointer(target: EventTarget, type: string, clientX: number): void {
    const event = new Event(type, { bubbles: true })
    Object.assign(event, { clientX, pointerId: 1 })
    target.dispatchEvent(event)
  }

  it('拖动检查器左缘 resizer 向左变宽，写入 rightWidth', async () => {
    harness = await mountWithProviders(
      <WorkbenchLayout left={<div />} center={<div />} right={<div />} />
    )
    // 展开态（beforeEach 里 rightCollapsed=false）必须渲染检查器与它的 resizer。
    const resizer = harness.host.querySelector<HTMLElement>('.console-inspector-resizer')
    expect(resizer).not.toBeNull()
    await act(async () => firePointer(resizer!, 'pointerdown', 800))
    await act(async () => firePointer(window, 'pointermove', 700))
    expect(useCodeSessionStore.getState().rightWidth).toBe(492)
    await act(async () => firePointer(window, 'pointermove', 900))
    expect(useCodeSessionStore.getState().rightWidth).toBe(292)
    // 收尾拖拽：不 detach 会把 move 监听器泄漏进后续用例。
    await act(async () => firePointer(window, 'pointerup', 900))
  })

  it('键盘按 ArrowLeft 让检查器变宽（变宽方向与拖拽一致）', async () => {
    harness = await mountWithProviders(
      <WorkbenchLayout left={<div />} center={<div />} right={<div />} />
    )
    const resizer = harness.host.querySelector<HTMLElement>('.console-inspector-resizer')!
    await act(async () => {
      resizer.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }))
    })
    expect(useCodeSessionStore.getState().rightWidth).toBe(408)
  })

  it('左列头部收起按钮把 leftCollapsed 置真，边轨按钮恢复', async () => {
    harness = await mountWithProviders(
      <WorkbenchLayout left={<div />} center={<div />} right={<div />} />
    )
    const collapse = harness.host.querySelector<HTMLButtonElement>('.console-sidebar-collapse')
    expect(collapse).not.toBeNull()
    await act(async () => {
      collapse!.click()
    })
    expect(useCodeSessionStore.getState().leftCollapsed).toBe(true)
    // 收起后左列消失，只剩 26px 边轨按钮。
    expect(harness.host.querySelector('.console-col--left')).toBeNull()
    const tab = harness.host.querySelector<HTMLButtonElement>('.console-sidebar-tab')
    expect(tab).not.toBeNull()
    await act(async () => {
      tab!.click()
    })
    expect(useCodeSessionStore.getState().leftCollapsed).toBe(false)
    expect(harness.host.querySelector('.console-col--left')).not.toBeNull()
  })

  it('检查器收起时不渲染 resizer（无宽度可调）', async () => {
    useCodeSessionStore.setState({ rightCollapsed: true })
    harness = await mountWithProviders(
      <WorkbenchLayout left={<div />} center={<div />} right={<div />} />
    )
    expect(harness.host.querySelector('.console-inspector-resizer')).toBeNull()
  })
})
