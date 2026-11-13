/**
 * Export behavior of `threads-api.exportThread`:
 * desktop shell → folder picker + shell-side save; browser → blob download.
 *
 * @vitest-environment jsdom
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const desktopMocks = vi.hoisted(() => ({
  isDesktopShell: vi.fn(() => false),
  pickFolder: vi.fn(async () => null as string | null),
  saveFile: vi.fn(async (_path: string, _bytes: Uint8Array) => '')
}))

vi.mock('../../../shared/desktop', () => desktopMocks)

import { exportThread } from './threads-api'

const THREAD_ID = '00000000-0000-0000-0000-000000000002'
const ZIP_BYTES = new Uint8Array([0x50, 0x4b, 0x03, 0x04, 1, 2, 3])

/** Plain-object response: jsdom's Blob lacks `arrayBuffer()`, undici's does not
 * matter here — exportThread only reads ok/status/blob(). */
function zipResponse(): Response {
  return {
    ok: true,
    status: 200,
    blob: async () => ({
      arrayBuffer: async () => ZIP_BYTES.slice().buffer as ArrayBuffer
    })
  } as unknown as Response
}

beforeEach(() => {
  desktopMocks.isDesktopShell.mockReturnValue(false)
  desktopMocks.pickFolder.mockResolvedValue(null)
  desktopMocks.saveFile.mockResolvedValue('')
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('exportThread', () => {
  it('请求 ?format=zip 并在桌面壳层中把 zip 保存到用户选择的目录', async () => {
    desktopMocks.isDesktopShell.mockReturnValue(true)
    desktopMocks.pickFolder.mockResolvedValue('/Users/me/Exports')
    const fetchMock = vi.fn(async (_url: string | URL) => zipResponse())
    vi.stubGlobal('fetch', fetchMock)

    await exportThread(THREAD_ID, '新会话')

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url] = fetchMock.mock.calls[0] ?? ['']
    expect(url).toBe(`/v1/threads/${THREAD_ID}/export?format=zip`)

    expect(desktopMocks.pickFolder).toHaveBeenCalledTimes(1)
    expect(desktopMocks.saveFile).toHaveBeenCalledTimes(1)
    const [path, bytes] = desktopMocks.saveFile.mock.calls[0] ?? ['', new Uint8Array()]
    expect(path).toBe(`/Users/me/Exports/新会话-${THREAD_ID}.zip`)
    expect(Array.from(bytes)).toEqual(Array.from(ZIP_BYTES))
  })

  it('用户在目录选择器里点取消时不落盘', async () => {
    desktopMocks.isDesktopShell.mockReturnValue(true)
    desktopMocks.pickFolder.mockResolvedValue(null)
    const fetchMock = vi.fn(async (_url: string | URL) => zipResponse())
    vi.stubGlobal('fetch', fetchMock)

    await exportThread(THREAD_ID, 'thread')

    expect(desktopMocks.pickFolder).toHaveBeenCalledTimes(1)
    expect(desktopMocks.saveFile).not.toHaveBeenCalled()
  })

  it('非桌面环境回退为浏览器下载（blob + anchor）', async () => {
    const fetchMock = vi.fn(async (_url: string | URL) => zipResponse())
    vi.stubGlobal('fetch', fetchMock)
    const createObjectURL = vi.fn(() => 'blob:mock')
    const revokeObjectURL = vi.fn()
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL, revokeObjectURL }))
    const click = vi.fn()
    const originalCreate = document.createElement.bind(document)
    vi.spyOn(document, 'createElement').mockImplementation((tag, options) => {
      const element = originalCreate(tag, options)
      if (tag === 'a') {
        Object.defineProperty(element, 'click', { value: click })
      }
      return element
    })

    await exportThread(THREAD_ID, 'thread')

    expect(desktopMocks.pickFolder).not.toHaveBeenCalled()
    expect(desktopMocks.saveFile).not.toHaveBeenCalled()
    expect(createObjectURL).toHaveBeenCalledTimes(1)
    expect(click).toHaveBeenCalledTimes(1)
  })
})
