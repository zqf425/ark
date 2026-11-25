// @vitest-environment jsdom
/**
 * Files must land as attachments whether they enter on the textarea or on any
 * other part of the composer box (chips strip, action rail, padding). The drop
 * target is therefore the whole `.ark-composer__box`, and a `Files` drag-over
 * must cancel the browser default everywhere on the box so a missed release
 * never opens the file in the webview.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { Composer } from './Composer'
import { uploadAttachment } from '../api/attachments-api'

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
      return {
        send: vi.fn(async () => 'accepted'),
        steer: vi.fn(async () => 'accepted'),
        interrupt: vi.fn(async () => undefined),
        canSend: Boolean(threadId) && !sending,
        canInterrupt: false
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
    base_skills: [],
    extra_skills: [],
    warnings: [],
    base_validation_errors: []
  })),
  refreshThreadSkills: vi.fn()
}))

const THREAD_A = '00000000-0000-0000-0000-00000000000a'

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
}

function dropFiles(files: File[]): DragEvent {
  const event = new Event('drop', { bubbles: true, cancelable: true }) as DragEvent
  Object.defineProperty(event, 'dataTransfer', {
    value: { files, types: ['Files'] },
    configurable: true
  })
  return event
}

function filesDragEvent(): DragEvent {
  const event = new Event('dragover', { bubbles: true, cancelable: true }) as DragEvent
  Object.defineProperty(event, 'dataTransfer', {
    value: { files: [], types: ['Files'] },
    configurable: true
  })
  return event
}

describe('Composer box-wide drop target', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    vi.mocked(uploadAttachment).mockReset()
    useCodeSessionStore.setState({ drafts: {}, sending: false, streamStatus: 'open', activeTurnId: null })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('a drop on the composer box (not only the textarea) uploads the file', async () => {
    mockUpload('00000000-0000-0000-0000-0000000000d1', 'notes.md')
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)

    const box = harness.host.querySelector<HTMLElement>('.ark-composer__box')
    expect(box).not.toBeNull()
    await act(async () => {
      box!.dispatchEvent(dropFiles([new File(['data'], 'notes.md', { type: 'text/markdown' })]))
      await Promise.resolve()
      await Promise.resolve()
    })
    await settle()

    expect(uploadAttachment).toHaveBeenCalledTimes(1)
    expect(harness.host.querySelector('.ark-composer__attachments')).not.toBeNull()
  })

  it('a Files dragover on the box is cancelled so the webview never opens the file', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)

    const box = harness.host.querySelector<HTMLElement>('.ark-composer__box')
    const event = filesDragEvent()
    await act(async () => {
      box!.dispatchEvent(event)
      await Promise.resolve()
    })
    await settle()

    expect(event.defaultPrevented).toBe(true)
    expect(box!.className).toContain('is-dragover')
  })

  it('a text-only dragover keeps the default and never highlights the box', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)

    const box = harness.host.querySelector<HTMLElement>('.ark-composer__box')
    const event = new Event('dragover', { bubbles: true, cancelable: true }) as DragEvent
    Object.defineProperty(event, 'dataTransfer', {
      value: { files: [], types: ['text/plain'] },
      configurable: true
    })
    await act(async () => {
      box!.dispatchEvent(event)
      await Promise.resolve()
    })
    await settle()

    expect(event.defaultPrevented).toBe(false)
    expect(box!.className).not.toContain('is-dragover')
  })

  it('a drop carrying no files is ignored instead of erroring', async () => {
    harness = await mountWithProviders(<Composer threadId={THREAD_A} />)

    const box = harness.host.querySelector<HTMLElement>('.ark-composer__box')
    await act(async () => {
      box!.dispatchEvent(dropFiles([]))
      await Promise.resolve()
    })
    await settle()

    expect(uploadAttachment).not.toHaveBeenCalled()
  })
})
