// @vitest-environment jsdom
/**
 * The new-task page mounts the session composer with no thread and no mode
 * picker. Its send action is a pending start: create the thread, commit the
 * staged runtime picks, associate the picked attachments and start the first
 * turn — exactly one delivery. A delivered send jumps into the workbench
 * (view switch to `code`) with the new thread already active in the store; a
 * failed create must keep the draft and the attachments so the next attempt
 * still has everything, and the staged workspace rides the create call
 * (server-side validation), not a PATCH after the fact.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../../code-session/state/store'
import { NEW_TASK_DRAFT_KEY } from '../../code-session/components/Composer'
import { useViewStore } from '../../workbench/state/view-slice'
import { NewTaskPage } from './NewTaskPage'

vi.mock('../../code-session/api/threads-api', () => ({
  createThread: vi.fn(),
  updateThread: vi.fn()
}))

vi.mock('../../code-session/api/turns-api', () => ({
  startTurn: vi.fn()
}))

vi.mock('../../code-session/api/attachments-api', () => ({
  uploadAttachment: vi.fn(),
  associateAttachment: vi.fn(),
  fetchAttachmentObjectUrl: vi.fn(async () => 'blob:mock')
}))

vi.mock('../../code-session/components/ComposerRuntimeControls', () => ({
  AutonomyPicker: () => null,
  WorkspacePicker: () => null,
  ContextRing: () => null,
  ModelPicker: () => null
}))

vi.mock('../../code-session/api/skills-api', () => ({
  listSkills: vi.fn(async () => ({ skills: [] })),
  listThreadSkills: vi.fn(async () => ({
    enabled: true,
    base_roots: [],
    extra_roots: [],
    loaded_extra_roots: [],
    base_skills: [],
    extra_skills: [],
    warnings: [],
    base_validation_errors: []
  })),
  refreshThreadSkills: vi.fn()
}))

import {
  createThread,
  updateThread
} from '../../code-session/api/threads-api'
import { startTurn } from '../../code-session/api/turns-api'
import { associateAttachment } from '../../code-session/api/attachments-api'

const THREAD = '00000000-0000-0000-0000-0000000000n1'

function input(host: HTMLElement): HTMLTextAreaElement {
  const node = host.querySelector<HTMLTextAreaElement>('.ark-composer__input')
  if (!node) throw new Error('composer textarea not mounted')
  return node
}

async function type(node: HTMLTextAreaElement, text: string): Promise<void> {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set
  if (!setter) throw new Error('native value setter unavailable')
  setter.call(node, text)
  node.dispatchEvent(new Event('input', { bubbles: true }))
  await settle()
}

async function pressEnter(node: HTMLTextAreaElement): Promise<void> {
  node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
  // One settled microtask turn keeps act() calls from overlapping: settle()
  // below owns the rest of the queue fan-out.
  await act(async () => {
    await Promise.resolve()
  })
}

describe('NewTaskPage composer delivery', () => {
  let harness: ReactHarness | null = null
  // The delivery chain (create → PATCH → associate → start → store writes →
  // view switch) resolves across many microtasks after Enter; each helper turn
  // stays inside act via `settle`, but React still logs its own bookkeeping
  // warnings for the store-driven updates. The assertions below are on the
  // final state, so the noise is expected test-only output.
  beforeEach(() => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  beforeEach(() => {
    installBrowserStubs()
    useCodeSessionStore.setState({ drafts: {}, sending: false, streamStatus: 'closed', activeTurnId: null })
    useViewStore.setState({ view: 'new' })
    vi.mocked(createThread).mockReset()
    vi.mocked(updateThread).mockReset()
    vi.mocked(startTurn).mockReset()
    vi.mocked(associateAttachment).mockReset()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('the composer renders its full toolbar before any thread exists and no mode cards remain', async () => {
    harness = await mountWithProviders(<NewTaskPage meta={undefined} />)

    // The textarea is live without a thread (the page owns delivery), and the
    // round + trigger (attachment menu) is mounted and enabled.
    const node = input(harness.host)
    expect(node.disabled).toBe(false)
    const plus = harness.host.querySelector<HTMLButtonElement>('.ark-composer__round')
    expect(plus).not.toBeNull()
    expect(plus!.disabled).toBe(false)
    // The send button exists with the page's primary action label.
    const send = harness.host.querySelector<HTMLButtonElement>('.ark-composer__sendbtn')
    expect(send).not.toBeNull()
    expect(send!.getAttribute('aria-label')).toBe('开始任务')
    // The Code/Plan/Review mode cards are gone from the page.
    expect(harness.host.querySelector('.console-new__modes')).toBeNull()
  })

  it('Enter creates the thread, starts the first turn and lands on the canvas', async () => {
    vi.mocked(createThread).mockResolvedValue({
      thread_id: THREAD,
      title: '新会话',
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      archived: false,
      running: false
    })
    vi.mocked(startTurn).mockResolvedValue({
      thread_id: THREAD,
      turn_id: '00000000-0000-0000-0000-0000000000t1',
      state: 'running',
      started_at: new Date().toISOString()
    })

    harness = await mountWithProviders(<NewTaskPage meta={undefined} />)
    const node = input(harness.host)
    await type(node, '把工具超时重试收敛到 ark-agent')
    // The delivery runs create → PATCH → associate → start; keep the chain
    // inside act so the interleaved state updates stay wrapped.
    await pressEnter(node)
    await settle()

    expect(createThread).toHaveBeenCalledTimes(1)
    expect(startTurn).toHaveBeenCalledWith(THREAD, { text: '把工具超时重试收敛到 ark-agent', attachment_ids: [] })
    expect(useCodeSessionStore.getState().activeThreadId).toBe(THREAD)
    // The delivered draft is pruned; nothing is left for a phantom resend.
    expect(useCodeSessionStore.getState().drafts[NEW_TASK_DRAFT_KEY]).toBeUndefined()
    // The send jumps into the workbench: the shell's scene switches to `code`
    // with the new thread already active, opening it in the session list.
    expect(useViewStore.getState().view).toBe('code')
    // No staged overrides → no runtime PATCH.
    expect(updateThread).not.toHaveBeenCalled()
  })

  it('coalesces duplicate submits in the same event window into one create/start', async () => {
    let resolveCreate!: (value: unknown) => void
    vi.mocked(createThread).mockReturnValue(new Promise((resolve) => { resolveCreate = resolve }) as never)
    vi.mocked(startTurn).mockResolvedValue({
      thread_id: THREAD,
      turn_id: '00000000-0000-0000-0000-0000000000t1',
      state: 'running',
      started_at: new Date().toISOString()
    })
    harness = await mountWithProviders(<NewTaskPage meta={undefined} />)
    const node = input(harness.host)
    await type(node, '只能创建一次')
    await act(async () => {
      node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
      node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
      await Promise.resolve()
    })
    expect(createThread).toHaveBeenCalledTimes(1)
    await act(async () => {
      resolveCreate({
        thread_id: THREAD, title: '新会话', created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(), archived: false, running: false
      })
      await Promise.resolve()
    })
    await settle()
    expect(startTurn).toHaveBeenCalledTimes(1)
  })

  it('a failed create keeps the draft so the next attempt still has it', async () => {
    vi.mocked(createThread).mockRejectedValue(new Error('boom'))

    harness = await mountWithProviders(<NewTaskPage meta={undefined} />)
    const node = input(harness.host)
    await type(node, '不能丢的草稿')
    // The rejected create routes through the component's own catch (toast +
    // redelivery state); one extra act turn keeps React's bookkeeping wrapped.
    await pressEnter(node)
    await settle()

    expect(startTurn).not.toHaveBeenCalled()
    expect(useCodeSessionStore.getState().drafts[NEW_TASK_DRAFT_KEY]).toBe('不能丢的草稿')
    // A failed delivery never switches the scene: the user stays on the
    // new-task page with draft and picks intact for the next attempt.
    expect(useViewStore.getState().view).toBe('new')
  })

  it('a picked attachment is associated after the thread exists and rides the turn', async () => {
    vi.mocked(createThread).mockResolvedValue({
      thread_id: THREAD,
      title: '新会话',
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      archived: false,
      running: false
    })
    vi.mocked(startTurn).mockResolvedValue({
      thread_id: THREAD,
      turn_id: '00000000-0000-0000-0000-0000000000t2',
      state: 'running',
      started_at: new Date().toISOString()
    })
    vi.mocked(associateAttachment).mockResolvedValue(undefined)
    const { uploadAttachment } = await import('../../code-session/api/attachments-api')
    vi.mocked(uploadAttachment).mockResolvedValue({
      attachment: {
        id: '00000000-0000-0000-0000-0000000000p1',
        original_filename: 'pic.png',
        byte_size: 4,
        created_at: new Date().toISOString(),
        sha256: ''
      }
    })

    harness = await mountWithProviders(<NewTaskPage meta={undefined} />)
    // The composer is editable before any thread exists; the file input is live.
    const fileInput = harness.host.querySelector<HTMLInputElement>('input[type="file"]')
    expect(fileInput).not.toBeNull()
    const file = new File(['data'], 'pic.png', { type: 'image/png' })
    Object.defineProperty(fileInput, 'files', { value: [file], configurable: true })
    await act(async () => {
      fileInput!.dispatchEvent(new Event('change', { bubbles: true }))
      await Promise.resolve()
      await Promise.resolve()
    })
    await settle()
    await act(async () => {})

    expect(harness.host.querySelector('.ark-composer__attachments .ark-thumb--image')).not.toBeNull()

    const node = input(harness.host)
    await type(node, '看看这张截图')
    await pressEnter(node)
    await settle()

    expect(associateAttachment).toHaveBeenCalledWith(THREAD, '00000000-0000-0000-0000-0000000000p1')
    expect(startTurn).toHaveBeenCalledWith(THREAD, {
      text: '看看这张截图',
      attachment_ids: ['00000000-0000-0000-0000-0000000000p1']
    })
  })
})
