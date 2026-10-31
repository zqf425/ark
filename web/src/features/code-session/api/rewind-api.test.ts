import { afterEach, describe, expect, it, vi } from 'vitest'
import { rewindThread } from './rewind-api'

const THREAD_ID = '00000000-0000-0000-0000-000000000001'
const TURN_ID = '00000000-0000-0000-0000-0000000000aa'

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('rewindThread', () => {
  it('POST /v1/threads/{id}/rewind，body 只带 turn_id', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ last_seq: 7, rewind_from_seq: 3, already_rewound: false }), {
        status: 200,
        headers: { 'content-type': 'application/json' }
      })
    )
    vi.stubGlobal('fetch', fetchMock)

    const result = await rewindThread(THREAD_ID, { turn_id: TURN_ID })

    expect(result.last_seq).toBe(7)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe(`/v1/threads/${THREAD_ID}/rewind`)
    expect(init.method).toBe('POST')
    expect(JSON.parse(String(init.body))).toEqual({ turn_id: TURN_ID })
  })
})
