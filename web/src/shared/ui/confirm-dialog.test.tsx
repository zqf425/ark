// @vitest-environment jsdom
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../testing/react-harness'
import { ConfirmDialog } from './ConfirmDialog'

describe('ConfirmDialog', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('resolves false on cancel and true on confirm', async () => {
    const onResolve = vi.fn()
    harness = await mountWithProviders(
      <ConfirmDialog title="删除项目？" confirmLabel="删除" cancelLabel="取消" detail="此操作不可恢复。" onResolve={onResolve} />
    )
    await settle()

    const buttons = harness.host.querySelectorAll<HTMLButtonElement>('.ark-confirmdialog__actions button')
    expect(buttons.length).toBe(2)
    expect(buttons[0]!.textContent).toBe('取消')
    expect(buttons[buttons.length - 1]!.textContent).toBe('删除')

    await act(async () => { buttons[0]!.click() })
    expect(onResolve).toHaveBeenCalledWith(false)

    await act(async () => { buttons[buttons.length - 1]!.click() })
    expect(onResolve).toHaveBeenCalledWith(true)
  })

  it('lands initial focus on cancel and resolves false on Escape so the gate fails closed', async () => {
    const onResolve = vi.fn()
    harness = await mountWithProviders(
      <ConfirmDialog title="删除项目？" confirmLabel="删除" cancelLabel="取消" onResolve={onResolve} />
    )
    await settle()

    expect(document.activeElement?.className).toContain('ark-btn--ghost')
    // Escape is wired through the panel's React onKeyDown (useDialogFocus), not
    // a window listener, so the event must target the dialog surface itself.
    const panel = harness!.host.querySelector<HTMLElement>('.ark-confirmdialog__panel')!
    await act(async () => {
      panel.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    })
    expect(onResolve).toHaveBeenCalledWith(false)
  })

  it('resolves false when the scrim is clicked', async () => {
    const onResolve = vi.fn()
    harness = await mountWithProviders(
      <ConfirmDialog title="删除项目？" confirmLabel="删除" cancelLabel="取消" onResolve={onResolve} />
    )
    await settle()

    await act(async () => {
      harness!.host.querySelector<HTMLButtonElement>('.ark-confirmdialog__scrim')!.click()
    })
    expect(onResolve).toHaveBeenCalledWith(false)
  })
})
