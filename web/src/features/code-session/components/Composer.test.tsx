// @vitest-environment jsdom
/**
 * The draft must survive thread switches. `ThreadStream` remounts the Composer
 * per thread (`key={threadId}`), so the textarea alone cannot hold the draft:
 * the store keeps one entry per thread and the remount restores it. The same
 * tests pin the complement — a sent draft is pruned, and queued text never
 * leaks into another thread's input.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { Composer } from './Composer'

vi.mock('./ComposerRuntimeControls', () => ({
  AutonomyPicker: () => null,
  WorkspacePicker: () => null,
  ContextRing: () => null,
  ModelPicker: () => null
}))

vi.mock('../hooks/use-turn', async () => {
  const { useCodeSessionStore } = await import('../state/store')
  return {
    useTurn: (threadId: string | null) => {
      const sending = useCodeSessionStore((s) => s.sending)
      const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
      return {
        send: vi.fn(async () => true),
        steer: vi.fn(async () => true),
        interrupt: vi.fn(async () => undefined),
        canSend: Boolean(threadId) && !sending,
        canInterrupt: Boolean(threadId) && Boolean(activeTurnId) && !sending
      }
    }
  }
})

vi.mock('../api/attachments-api', () => ({
  uploadAttachment: vi.fn(),
  associateAttachment: vi.fn()
}))

vi.mock('../api/skills-api', () => ({
  listSkills: vi.fn(async () => ({ skills: [] })),
  listThreadSkills: vi.fn(),
  refreshThreadSkills: vi.fn()
}))

const THREAD_A = '00000000-0000-0000-0000-00000000000a'
const THREAD_B = '00000000-0000-0000-0000-00000000000b'

function input(host: HTMLElement): HTMLTextAreaElement {
  const node = host.querySelector<HTMLTextAreaElement>('.ark-composer__input')
  if (!node) throw new Error('composer textarea not mounted')
  return node
}

/**
 * Drive typing the way React observes it: write through the native value
 * setter (React hooks the instance property and would swallow the change),
 * then dispatch `input`, which React delegates at the root.
 */
async function type(node: HTMLTextAreaElement, text: string): Promise<void> {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set
  if (!setter) throw new Error('native value setter unavailable')
  setter.call(node, text)
  node.dispatchEvent(new Event('input', { bubbles: true }))
  await settle()
}

async function pressEnter(node: HTMLTextAreaElement): Promise<void> {
  node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
  await settle()
}

/** Click with the state fan wrapped in act() so React does not warn. */
async function click(node: HTMLElement): Promise<void> {
  await act(async () => {
    node.click()
    await Promise.resolve()
  })
}

describe('Composer per-thread draft persistence', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    useCodeSessionStore.setState({ drafts: {}, sending: false, streamStatus: 'open', activeTurnId: null })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('keeps the draft when the component remounts for another thread and back', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    await type(input(harness.host), '给 A 会话的草稿')
    expect(useCodeSessionStore.getState().drafts[THREAD_A]).toBe('给 A 会话的草稿')

    // A real switch remounts the component (keyed by thread), so simulate it.
    await harness.render(<Composer key={THREAD_B} threadId={THREAD_B} />)
    expect(input(harness.host).value).toBe('')

    // Back to A: the draft must come back from the store.
    await harness.render(<Composer key={THREAD_A} threadId={THREAD_A} />)
    expect(input(harness.host).value).toBe('给 A 会话的草稿')
  })

  it('clears the thread draft after a successful send', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, '要发送的内容')
    await pressEnter(node)

    expect(useCodeSessionStore.getState().drafts[THREAD_A]).toBeUndefined()
    expect(node.value).toBe('')
  })

  it('keeps the draft when the send gate blocks (stream reconnecting)', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, '发送失败也要留住')
    useCodeSessionStore.setState({ streamStatus: 'reconnecting' })
    // Flush the re-render so the Enter handler sees the closed gate, the way
    // it always does in a real session (renders land between keystrokes).
    await settle()
    await pressEnter(node)

    expect(node.value).toBe('发送失败也要留住')
    expect(useCodeSessionStore.getState().drafts[THREAD_A]).toBe('发送失败也要留住')
  })

  it('a queued draft clears the input and the store draft', async () => {
    // canInterrupt mirrors a live turn, which is what unlocks the queue action.
    useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, '排队中的消息')

    // Typing during a live turn swaps the interrupt button for the delivery
    // chooser; switching the mode to queue turns Enter into a queue action.
    expect(harness.host.querySelector('.ark-composer__round--stop')).toBeNull()
    const trigger = harness.host.querySelector<HTMLButtonElement>('.ark-composer__delivery-trigger')
    expect(trigger).toBeDefined()
    await click(trigger!)
    await settle()
    const queueOption = [...harness.host.querySelectorAll<HTMLButtonElement>('.ark-composer__delivery-option')].find(
      (option) => option.textContent?.includes('排队')
    )
    expect(queueOption).toBeDefined()
    await click(queueOption!)
    await settle()
    await pressEnter(node)

    // The text moved into the local queue chip; nothing stays in the draft.
    expect(node.value).toBe('')
    expect(useCodeSessionStore.getState().drafts[THREAD_A]).toBeUndefined()
    expect(harness.text()).toContain('#1 排队')
  })

  it('an empty running composer shows the interrupt button instead of the chooser', async () => {
    useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)

    // No draft: the rightmost control is the stop button; the chooser and the
    // send button are both gone.
    expect(harness.host.querySelector('.ark-composer__round--stop')).toBeDefined()
    expect(harness.host.querySelector('.ark-composer__delivery')).toBeNull()
    expect(harness.host.querySelector('.ark-composer__sendbtn')).toBeNull()
  })

  it('enter steers during a live turn by default and keeps the send button for idle', async () => {
    useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, '运行中的补充指令')

    // The chooser defaults to steer; Enter routes to steer(), not send().
    await pressEnter(node)
    await settle()
    expect(useCodeSessionStore.getState().drafts[THREAD_A]).toBeUndefined()
    expect(harness.host.querySelector('.ark-composer__sendbtn')).toBeNull()

    // The turn ends: the send button is back and the stop button is gone.
    useCodeSessionStore.setState({ activeTurnId: null })
    await settle()
    expect(harness.host.querySelector('.ark-composer__sendbtn')).toBeDefined()
    expect(harness.host.querySelector('.ark-composer__round--stop')).toBeNull()
  })
})
