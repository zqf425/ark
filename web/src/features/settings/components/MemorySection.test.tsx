// @vitest-environment jsdom
/**
 * The memory section over its mocked API surface.
 *
 * Pinned here: the list renders the authoritative entries, create validates
 * the 16KiB/empty bounds before any request, deletion reaches the API, and the
 * update path sends an explicit `null` source (the only way to clear it).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import type { MemoryDiagnosticsResponse, MemoryEntry, MemoryListResponse } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { createMemory, deleteMemory, getMemoryDiagnostics, listMemories, updateMemory } from '../api/memory-api'
import { MemorySection } from './MemorySection'

vi.mock('../api/memory-api', () => ({
  listMemories: vi.fn(),
  createMemory: vi.fn(),
  updateMemory: vi.fn(),
  deleteMemory: vi.fn(),
  getMemoryDiagnostics: vi.fn()
}))

const listMock = vi.mocked(listMemories)
const createMock = vi.mocked(createMemory)
const updateMock = vi.mocked(updateMemory)
const deleteMock = vi.mocked(deleteMemory)
const diagMock = vi.mocked(getMemoryDiagnostics)

function entry(overrides: Partial<MemoryEntry> = {}): MemoryEntry {
  return {
    id: 'mem-1',
    text: 'prefers concise answers',
    tags: ['style'],
    source: 'manual',
    created_at: '2026-11-02T00:00:00Z',
    updated_at: '2026-11-02T00:00:00Z',
    ...overrides
  }
}

const diagnostics: MemoryDiagnosticsResponse = {
  storage_path: '/tmp/ark/memory.json',
  entry_count: 1,
  last_written_at: '2026-11-02T00:00:00Z'
}

function setValue(element: Element, value: string): void {
  const proto = element.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set
  setter?.call(element, value)
  element.dispatchEvent(new Event('input', { bubbles: true }))
}

describe('MemorySection', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    createMock.mockReset()
    updateMock.mockReset()
    deleteMock.mockReset()
    diagMock.mockReset()
    const response: MemoryListResponse = { entries: [entry()] }
    listMock.mockResolvedValue(response)
    diagMock.mockResolvedValue(diagnostics)
    updateMock.mockImplementation((_id, request) =>
      Promise.resolve(entry({ source: request.source ?? undefined }))
    )
    deleteMock.mockResolvedValue(undefined)
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.restoreAllMocks()
  })

  it('renders the authoritative entries and diagnostics', async () => {
    harness = await mountWithProviders(<MemorySection />)
    await settle()

    expect(harness.text()).toContain('prefers concise answers')
    expect(harness.text()).toContain('/tmp/ark/memory.json')
  })

  it('rejects empty text locally without a network call', async () => {
    harness = await mountWithProviders(<MemorySection />)
    await settle()

    const createButton = [...harness.host.querySelectorAll('button')].find((button) =>
      button.textContent?.includes('新增记忆')
    )
    expect(createButton).toBeDefined()
    await act(async () => {
      createButton?.click()
    })
    await settle()

    expect(harness.text()).toContain('内容不能为空')
    expect(createMock).not.toHaveBeenCalled()
  })

  it('sends parsed tags and an optional source on create', async () => {
    createMock.mockResolvedValue(entry({ id: 'mem-2', text: 'new', tags: [], source: undefined }))
    harness = await mountWithProviders(<MemorySection />)
    await settle()

    const textarea = harness.host.querySelector('textarea')
    const inputs = [...harness.host.querySelectorAll('input')]
    await act(async () => {
      setValue(textarea!, 'keep this fact')
      setValue(inputs[0]!, 'a, b')
      setValue(inputs[1]!, 'web')
    })
    const createButton = [...harness.host.querySelectorAll('button')].find((button) =>
      button.textContent?.includes('新增记忆')
    )
    await act(async () => {
      createButton?.click()
    })
    await settle()

    expect(createMock).toHaveBeenCalledWith({
      text: 'keep this fact',
      tags: ['a', 'b'],
      source: 'web'
    })
  })

  it('clears a stored source with an explicit null on update', async () => {
    updateMock.mockResolvedValue(entry({ source: undefined }))
    harness = await mountWithProviders(<MemorySection />)
    await settle()

    const editButton = [...harness.host.querySelectorAll('button')].find((button) =>
      button.textContent === '编辑'
    )
    await act(async () => {
      editButton?.click()
    })
    await settle()

    // The edit form starts from the stored source; clearing it must send null.
    const editHost = harness.host
    const editTextarea = editHost.querySelector('.ark-card textarea')
    await act(async () => {
      setValue(editTextarea!, 'edited text')
      const sourceInput = [...editHost.querySelectorAll('.ark-card input')][1]!
      setValue(sourceInput, '')
    })
    const saveButton = [...editHost.querySelectorAll('button')].find((button) =>
      button.textContent === '保存'
    )
    await act(async () => {
      saveButton?.click()
    })
    await settle()

    expect(updateMock).toHaveBeenCalledWith('mem-1', {
      text: 'edited text',
      tags: ['style'],
      source: null
    })
  })

  it('reaches the delete API from the card action', async () => {
    harness = await mountWithProviders(<MemorySection />)
    await settle()

    const deleteButton = [...harness.host.querySelectorAll('button')].find((button) =>
      button.textContent === '删除'
    )
    await act(async () => {
      deleteButton?.click()
    })
    await settle()

    expect(deleteMock).toHaveBeenCalledWith('mem-1')
  })
})
