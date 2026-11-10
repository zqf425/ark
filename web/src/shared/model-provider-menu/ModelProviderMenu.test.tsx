// @vitest-environment jsdom
/**
 * Every model-selection surface now uses this one two-level menu, so the
 * interaction contract is pinned here: a single trigger, the provider column
 * switching the model column, an atomic (providerId, model) selection, the
 * "follow default" clear entry, a default preview, synthetic rows for stale
 * ids, and dismissal in both controlled and uncontrolled modes.
 */
import { act, type ComponentProps, type ReactElement } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, type ReactHarness } from '../testing/react-harness'
import { ModelProviderMenu } from './ModelProviderMenu'
import type { ModelProviderGroup } from './groups'

const GROUPS: ModelProviderGroup[] = [
  { id: 'p1', label: 'OpenAI', models: ['gpt-5', 'gpt-4o'] },
  { id: 'p2', label: 'DeepSeek', models: ['deepseek-chat'] }
]

function renderMenu(
  overrides: Partial<ComponentProps<typeof ModelProviderMenu>> = {}
): ReactElement {
  return (
    <ModelProviderMenu
      groups={GROUPS}
      value={{ providerId: '', model: '' }}
      onSelect={() => undefined}
      ariaLabel="选择供应商与模型"
      placeholder="选择模型"
      {...overrides}
    />
  )
}

function trigger(host: HTMLElement): HTMLButtonElement {
  const node = host.querySelector('button.ark-mpm__trigger')
  if (!node) throw new Error('trigger missing')
  return node as HTMLButtonElement
}

function buttonWith(host: HTMLElement, selector: string, label: string): HTMLButtonElement {
  const node = [...host.querySelectorAll(selector)].find((item) => item.textContent?.includes(label))
  if (!node) throw new Error(`button ${label} missing in ${selector}`)
  return node as HTMLButtonElement
}

async function openMenu(harness: ReactHarness): Promise<void> {
  await act(async () => {
    trigger(harness.host).click()
  })
}

async function clickButton(harness: ReactHarness, selector: string, label: string): Promise<void> {
  await act(async () => {
    buttonWith(harness.host, selector, label).click()
  })
}

function rightColumn(host: HTMLElement): HTMLElement {
  const node = host.querySelector('.ark-mpm__right')
  if (!node) throw new Error('model column missing')
  return node as HTMLElement
}

/** React tracks input values; a native setter is required to fire onChange. */
async function typeSearch(harness: ReactHarness, value: string): Promise<void> {
  const input = harness.host.querySelector('.ark-mpm__search input') as HTMLInputElement
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
  await act(async () => {
    setter?.call(input, value)
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
}

describe('ModelProviderMenu (two-level provider -> model)', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => installBrowserStubs())
  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('opens one menu and shows only the active provider\'s models', async () => {
    harness = await mountWithProviders(renderMenu())
    expect(harness.host.querySelector('.ark-mpm__menu')).toBeNull()
    await openMenu(harness)
    expect(harness.host.querySelector('.ark-mpm__menu')).not.toBeNull()
    const left = harness.host.querySelector('.ark-mpm__left') as HTMLElement
    expect(left.textContent).toContain('OpenAI')
    expect(left.textContent).toContain('DeepSeek')
    expect(rightColumn(harness.host).textContent).toContain('gpt-5')
    expect(rightColumn(harness.host).textContent).not.toContain('deepseek-chat')
  })

  it('swaps the model column when another provider row is chosen', async () => {
    harness = await mountWithProviders(renderMenu())
    await openMenu(harness)
    await clickButton(harness, '.ark-mpm__provider', 'DeepSeek')
    expect(rightColumn(harness.host).textContent).toContain('deepseek-chat')
    expect(rightColumn(harness.host).textContent).not.toContain('gpt-5')
  })

  it('reports the (providerId, model) pair atomically and closes', async () => {
    const onSelect = vi.fn()
    harness = await mountWithProviders(renderMenu({ onSelect }))
    await openMenu(harness)
    await clickButton(harness, '.ark-mpm__provider', 'DeepSeek')
    await clickButton(harness, '.ark-mpm__model', 'deepseek-chat')
    expect(onSelect).toHaveBeenCalledWith({ providerId: 'p2', model: 'deepseek-chat' })
    expect(harness.host.querySelector('.ark-mpm__menu')).toBeNull()
  })

  it('"follow default" clears the override and closes', async () => {
    const onClear = vi.fn()
    harness = await mountWithProviders(
      renderMenu({ clearSelection: { label: '跟随默认（留空）', onClear } })
    )
    await openMenu(harness)
    await clickButton(harness, '.ark-mpm__item--clear', '跟随默认')
    expect(onClear).toHaveBeenCalledTimes(1)
    expect(harness.host.querySelector('.ark-mpm__menu')).toBeNull()
  })

  it('previews the effective default and badges it when nothing is stored', async () => {
    harness = await mountWithProviders(
      renderMenu({ fallback: { providerId: 'p1', model: 'gpt-5' } })
    )
    expect(trigger(harness.host).textContent).toContain('OpenAI / gpt-5')
    expect(trigger(harness.host).textContent).toContain('默认')
  })

  it('shows a translated empty state when no providers exist', async () => {
    harness = await mountWithProviders(renderMenu({ groups: [] }))
    await openMenu(harness)
    const left = harness.host.querySelector('.ark-mpm__left') as HTMLElement
    expect(left.textContent).toContain('暂无供应商')
  })

  it('keeps a stored model that is no longer in the inventory selectable', async () => {
    harness = await mountWithProviders(
      renderMenu({ value: { providerId: 'p1', model: 'custom-x' } })
    )
    await openMenu(harness)
    expect(rightColumn(harness.host).textContent).toContain('custom-x')
  })

  it('forwards Escape dismissal to the parent in controlled mode', async () => {
    const onOpenChange = vi.fn()
    harness = await mountWithProviders(renderMenu({ open: true, onOpenChange }))
    expect(harness.host.querySelector('.ark-mpm__menu')).not.toBeNull()
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    })
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })

  it('filters the model column by search and reports no matches', async () => {
    harness = await mountWithProviders(renderMenu())
    await openMenu(harness)
    const input = harness.host.querySelector('.ark-mpm__search input') as HTMLInputElement
    expect(input).not.toBeNull()
    await typeSearch(harness, 'gpt-4o')
    const right = rightColumn(harness.host)
    expect(right.textContent).toContain('gpt-4o')
    expect(right.textContent).not.toContain('gpt-5')
    await typeSearch(harness, 'missing-model')
    expect(harness.host.querySelector('.ark-mpm__left')?.textContent).toContain('\u6ca1\u6709\u5339\u914d')
  })

  it('an unrelated parent re-render keeps the provider the operator switched to', async () => {
    const props = { fallback: { providerId: 'p1', model: 'gpt-5' } }
    harness = await mountWithProviders(renderMenu(props))
    await openMenu(harness)
    await clickButton(harness, '.ark-mpm__provider', 'DeepSeek')
    // A fresh `value` object, like a parent re-rendering with inline props.
    await harness.render(renderMenu({ fallback: { providerId: 'p1', model: 'gpt-5' } }))
    expect(rightColumn(harness.host).textContent).toContain('deepseek-chat')
  })
})
