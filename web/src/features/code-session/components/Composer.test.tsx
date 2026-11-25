// @vitest-environment jsdom
/**
 * The draft must survive thread switches. `ThreadStream` remounts the Composer
 * per thread (`key={threadId}`), so the textarea alone cannot hold the draft:
 * the store keeps one entry per thread and the remount restores it. The same
 * tests pin the complement — a sent draft is pruned, and queued text never
 * leaks into another thread's input. Attachment parity pins the steer channel:
 * a live turn accepts attachments (associated then steered) and a paste/drop
 * of files lands as attachments instead of inline text.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { Composer } from './Composer'
import { associateAttachment, uploadAttachment } from '../api/attachments-api'
import type { TurnSendResult } from '../hooks/use-turn'

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
        send: turnMocks.send,
        steer: turnMocks.steer,
        interrupt: vi.fn(async () => undefined),
        canSend: Boolean(threadId) && !sending,
        canInterrupt: Boolean(threadId) && Boolean(activeTurnId) && !sending
      }
    }
  }
})

vi.mock('../api/attachments-api', () => ({
  uploadAttachment: vi.fn(),
  associateAttachment: vi.fn(),
  fetchAttachmentObjectUrl: vi.fn(async () => 'blob:mock')
}))

vi.mock('../api/skills-api', () => ({
  listSkills: vi.fn(async () => ({ skills: [] })),
  listThreadSkills: vi.fn(async () => ({
    enabled: true,
    base_roots: [],
    extra_roots: [],
    loaded_extra_roots: [],
    base_skills: [
      {
        id: 'kun-new-session',
        name: 'kun 新会话',
        version: '0.1.0',
        root: '/skills/kun-new-session',
        entry_path: '/skills/kun-new-session/SKILL.md',
        priority: 0,
        source: 'base',
        legacy: false
      },
      {
        id: 'pdf',
        name: 'PDF 工具',
        version: '0.1.0',
        root: '/skills/pdf',
        entry_path: '/skills/pdf/SKILL.md',
        priority: 0,
        source: 'base',
        legacy: false
      }
    ],
    extra_skills: [],
    warnings: [],
    base_validation_errors: []
  })),
  refreshThreadSkills: vi.fn()
}))

const THREAD_A = '00000000-0000-0000-0000-00000000000a'
const THREAD_B = '00000000-0000-0000-0000-00000000000b'

/** Module-level stand-ins the use-turn mock hands to every consumer. */
const turnMocks = vi.hoisted(() => ({
  send: vi.fn<(text: string, attachmentIds: string[]) => Promise<TurnSendResult>>(async () => 'accepted'),
  steer: vi.fn<(text: string, attachmentIds: string[]) => Promise<TurnSendResult>>(async () => 'accepted')
}))

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
    useCodeSessionStore.setState({ drafts: {}, queuedMessages: {}, sending: false, streamStatus: 'open', activeTurnId: null })
    turnMocks.send.mockClear()
    turnMocks.steer.mockClear()
    turnMocks.send.mockResolvedValue('accepted')
    turnMocks.steer.mockResolvedValue('accepted')
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

  it('keeps each thread queue across keyed remounts without leaking it', async () => {
    useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, 'A 的排队消息')
    const trigger = harness.host.querySelector<HTMLButtonElement>('.ark-composer__delivery-trigger')!
    await click(trigger)
    await settle()
    const queueOption = [...harness.host.querySelectorAll<HTMLButtonElement>('.ark-composer__delivery-option')]
      .find((option) => option.textContent?.includes('排队'))!
    await click(queueOption)
    await settle()
    await pressEnter(node)
    expect(harness.text()).toContain('A 的排队消息')

    await harness.render(<Composer key={THREAD_B} threadId={THREAD_B} />)
    expect(harness.text()).not.toContain('A 的排队消息')
    await harness.render(<Composer key={THREAD_A} threadId={THREAD_A} />)
    expect(harness.text()).toContain('A 的排队消息')
  })

  it('keeps duplicate queued texts distinct and ignores a stale head completion', () => {
    const store = useCodeSessionStore.getState()
    store.queueMessage(THREAD_A, 'x')
    store.queueMessage(THREAD_A, 'x')
    const queue = useCodeSessionStore.getState().queuedMessages[THREAD_A]!
    const first = queue[0]!
    const second = queue[1]!

    expect(first.id).not.toBe(second.id)
    store.removeQueuedMessage(THREAD_A, first.id)
    store.dequeueMessage(THREAD_A, first.id)
    expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toEqual([second])

    store.dequeueMessage(THREAD_A, second.id)
    expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toBeUndefined()
  })

  it('retains a queued message when its automatic drain fails', async () => {
    useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
    turnMocks.send.mockResolvedValueOnce('retry')
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, '失败也不能丢')
    const trigger = harness.host.querySelector<HTMLButtonElement>('.ark-composer__delivery-trigger')!
    await click(trigger)
    await settle()
    const queueOption = [...harness.host.querySelectorAll<HTMLButtonElement>('.ark-composer__delivery-option')]
      .find((option) => option.textContent?.includes('排队'))!
    await click(queueOption)
    await settle()
    await pressEnter(node)
    useCodeSessionStore.setState({ activeTurnId: null })
    await settle()

    expect(turnMocks.send).toHaveBeenCalledWith('失败也不能丢', [])
    expect(harness.text()).toContain('失败也不能丢')
  })

  it('retries a failed idle drain without requiring another live-to-idle edge', async () => {
    vi.useFakeTimers()
    try {
      useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
      turnMocks.send.mockResolvedValueOnce('retry')
      useCodeSessionStore.getState().queueMessage(THREAD_A, '空闲时重试')
      harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
      await settle()

      await act(async () => {
        useCodeSessionStore.setState({ activeTurnId: null })
        await Promise.resolve()
        await Promise.resolve()
      })
      expect(turnMocks.send).toHaveBeenCalledTimes(1)
      expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toHaveLength(1)

      await act(async () => {
        vi.advanceTimersByTime(500)
        await Promise.resolve()
        await Promise.resolve()
      })
      expect(turnMocks.send).toHaveBeenCalledTimes(2)
      expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toBeUndefined()
    } finally {
      vi.useRealTimers()
    }
  })

  it('does not reschedule a permanently rejected queue head', async () => {
    vi.useFakeTimers()
    try {
      useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
      turnMocks.send.mockResolvedValue('permanent')
      useCodeSessionStore.getState().queueMessage(THREAD_A, '超过服务端上限的消息')
      harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
      await settle()

      await act(async () => {
        useCodeSessionStore.setState({ activeTurnId: null })
        await Promise.resolve()
        await Promise.resolve()
      })
      expect(turnMocks.send).toHaveBeenCalledTimes(1)
      expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toHaveLength(1)

      await act(async () => {
        vi.advanceTimersByTime(5_000)
        await Promise.resolve()
        await Promise.resolve()
      })
      expect(turnMocks.send).toHaveBeenCalledTimes(1)
      expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toHaveLength(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('pauses failed queue retries while the stream is closed and resumes after reopen', async () => {
    vi.useFakeTimers()
    try {
      useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
      turnMocks.send.mockResolvedValueOnce('retry')
      useCodeSessionStore.getState().queueMessage(THREAD_A, '重连后重试')
      harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
      await settle()

      await act(async () => {
        useCodeSessionStore.setState({ activeTurnId: null })
        await Promise.resolve()
        await Promise.resolve()
      })
      expect(turnMocks.send).toHaveBeenCalledTimes(1)

      await act(async () => {
        useCodeSessionStore.setState({ streamStatus: 'reconnecting' })
        vi.advanceTimersByTime(500)
        await Promise.resolve()
      })
      expect(turnMocks.send).toHaveBeenCalledTimes(1)
      expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toHaveLength(1)

      await act(async () => {
        useCodeSessionStore.setState({ streamStatus: 'open' })
        vi.advanceTimersByTime(500)
        await Promise.resolve()
        await Promise.resolve()
      })
      expect(turnMocks.send).toHaveBeenCalledTimes(2)
      expect(useCodeSessionStore.getState().queuedMessages[THREAD_A]).toBeUndefined()
    } finally {
      vi.useRealTimers()
    }
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

  it('typing / summons the skill palette and Enter completes the mention', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, '/')
    await settle()

    // The palette opens in token mode (no search field) with the loaded set.
    expect(harness.host.querySelector('.ark-skill-palette')).toBeDefined()
    expect(harness.host.querySelector('.ark-skill-palette__search')).toBeNull()
    expect(harness.text()).toContain('/kun-new-session')

    // Enter replaces the bare slash with the highlighted skill's full mention.
    await pressEnter(node)
    await settle()
    expect(useCodeSessionStore.getState().drafts[THREAD_A]).toBe('/kun-new-session ')
    // The palette closes once the token is gone.
    expect(harness.host.querySelector('.ark-skill-palette')).toBeNull()
  })

  it('the slash palette filters as the query grows and paths never summon it', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    const node = input(harness.host)
    await type(node, '/pd')
    await settle()
    expect(harness.host.querySelector('.ark-skill-palette')).toBeDefined()
    expect(harness.text()).toContain('/pdf')
    expect(harness.text()).not.toContain('/kun-new-session')

    // `foo/bar` is a path: the caret sits in a token whose `/` follows a word
    // character, so no palette. Typing on keeps the draft untouched.
    await type(node, 'foo/bar')
    await settle()
    expect(harness.host.querySelector('.ark-skill-palette')).toBeNull()
  })
})

describe('Composer attachment parity (steer, paste, drop)', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    turnMocks.send.mockClear()
    turnMocks.steer.mockClear()
    turnMocks.send.mockResolvedValue('accepted')
    turnMocks.steer.mockResolvedValue('accepted')
    vi.mocked(uploadAttachment).mockReset()
    vi.mocked(associateAttachment).mockReset()
    useCodeSessionStore.setState({ drafts: {}, queuedMessages: {}, sending: false, streamStatus: 'open', activeTurnId: null })
    turnMocks.send.mockClear()
    turnMocks.steer.mockClear()
    turnMocks.send.mockResolvedValue('accepted')
    turnMocks.steer.mockResolvedValue('accepted')
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  function mockUpload(id: string, name: string): void {
    vi.mocked(uploadAttachment).mockResolvedValue({
      attachment: {
        id,
        original_filename: name,
        byte_size: 4,
        created_at: new Date().toISOString(),
        sha256: ''
      }
    })
    vi.mocked(associateAttachment).mockResolvedValue(undefined)
  }

  async function pickPng(id: string, name = 'pic.png'): Promise<void> {
    mockUpload(id, name)
    const host = harness!.host
    const fileInput = host.querySelector<HTMLInputElement>('input[type="file"]')
    if (!fileInput) throw new Error('file input not mounted')
    const file = new File(['data'], name, { type: 'image/png' })
    Object.defineProperty(fileInput, 'files', { value: [file], configurable: true })
    await act(async () => {
      fileInput.dispatchEvent(new Event('change', { bubbles: true }))
      await Promise.resolve()
      await Promise.resolve()
    })
    await settle()
  }

  it('a live turn steers with the picked attachments and clears them after', async () => {
    useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    await pickPng('00000000-0000-0000-0000-0000000000p1')

    // The + trigger stays usable mid-turn: the steer channel carries files.
    expect(harness.host.querySelector('.ark-composer__round')).not.toBeNull()
    // The image lands as a clickable thumbnail, not a text chip.
    expect(harness.host.querySelector('.ark-composer__attachments .ark-thumb--image')).not.toBeNull()

    const node = input(harness.host)
    await type(node, '看看这张截图')
    await pressEnter(node)
    await settle()

    // Associated to the thread, then steered with the id; previews cleared.
    expect(associateAttachment).toHaveBeenCalledWith(THREAD_A, '00000000-0000-0000-0000-0000000000p1')
    expect(turnMocks.steer).toHaveBeenCalledWith('看看这张截图', ['00000000-0000-0000-0000-0000000000p1'])
    expect(harness.host.querySelector('.ark-composer__attachments')).toBeNull()
  })

  it('queueing keeps the attachments for the next steer or send', async () => {
    useCodeSessionStore.setState({ activeTurnId: '00000000-0000-0000-0000-0000000000t1' })
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    await pickPng('00000000-0000-0000-0000-0000000000p2')

    const node = input(harness.host)
    await type(node, '排队这条')
    // Switch the delivery mode to queue, then Enter queues the text only.
    const trigger = harness.host.querySelector<HTMLButtonElement>('.ark-composer__delivery-trigger')
    await click(trigger!)
    await settle()
    const queueOption = [...harness.host.querySelectorAll<HTMLButtonElement>('.ark-composer__delivery-option')].find(
      (option) => option.textContent?.includes('排队')
    )
    await click(queueOption!)
    await settle()
    await pressEnter(node)
    await settle()

    expect(harness.text()).toContain('#1 排队')
    // Nothing was associated or steered; the chips stay for the next delivery.
    expect(associateAttachment).not.toHaveBeenCalled()
    expect(harness.host.querySelector('.ark-composer__attachments')).not.toBeNull()
  })

  it('pasting an image uploads it as an attachment instead of inline text', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    mockUpload('00000000-0000-0000-0000-0000000000p3', 'pasted.png')

    const node = input(harness.host)
    const file = new File(['data'], 'pasted.png', { type: 'image/png' })
    const pasteEvent = new Event('paste', { bubbles: true, cancelable: true }) as ClipboardEvent
    Object.defineProperty(pasteEvent, 'clipboardData', {
      value: { files: [file] },
      configurable: true
    })
    await act(async () => {
      node.dispatchEvent(pasteEvent)
      await Promise.resolve()
      await Promise.resolve()
    })
    await settle()

    expect(uploadAttachment).toHaveBeenCalled()
    // The paste lands as an image thumbnail preview.
    expect(harness.host.querySelector('.ark-composer__attachments .ark-thumb--image')).not.toBeNull()
    // The draft is untouched: bytes ride the attachment, not the textarea.
    expect(useCodeSessionStore.getState().drafts[THREAD_A]).toBeUndefined()
  })

  it('clicking a picked image thumbnail opens the shared lightbox', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)
    await pickPng('00000000-0000-0000-0000-0000000000p4')

    const thumb = harness.host.querySelector<HTMLButtonElement>('.ark-composer__attachments .ark-thumb--image')
    expect(thumb).not.toBeNull()
    await act(async () => {
      thumb!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
    })
    await settle()
    expect(harness.host.querySelector('.ark-lightbox')).not.toBeNull()
  })
})
