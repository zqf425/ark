/**
 * The preview request must carry the session id
 * (requirements/workspace-file-preview.md).
 *
 * The runtime authorizes a preview against the *session's* effective workspace,
 * so a request without `thread_id` is authorized against the process-wide root
 * and can be rejected as "outside the workspace" for a session bound to another
 * project. This pins the query shape at the one place that builds it.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { getWorkspaceFilePreview } from './workbench-api'

function stubFetch(): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(() => Promise.resolve(new Response('{"path":"a.md"}', { status: 200 })))
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('getWorkspaceFilePreview', () => {
  it('sends the thread id with the preview path', async () => {
    const fetchMock = stubFetch()
    await getWorkspaceFilePreview('docs/a.md', 'thr-1')

    const url = String(fetchMock.mock.calls[0]?.[0])
    expect(url).toContain('/v1/workspace/file?')
    expect(url).toContain('path=docs%2Fa.md')
    expect(url).toContain('thread_id=thr-1')
  })

  it('keeps the thread id on an absolute Windows path', async () => {
    const fetchMock = stubFetch()
    await getWorkspaceFilePreview('D:\\code\\smart-ark\\a.md', 'thr-2')

    const url = String(fetchMock.mock.calls[0]?.[0])
    expect(url).toContain('thread_id=thr-2')
    expect(url).toContain('path=D%3A%5Ccode%5Csmart-ark%5Ca.md')
  })
})