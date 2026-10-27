/**
 * The only place allowed to touch `fetch` (design §7.10.2 / §7.5.3).
 *
 * Every runtime call goes through here so auth, error decoding and the SSE
 * framing exist exactly once. Business logic never lives in this file: callers
 * under `features/<x>/api` compose these primitives.
 */
import type { ArkErrorCode } from '../contracts'
import { ERROR_HINTS } from '../contracts'

const TOKEN_STORAGE_KEY = 'ark_token'
const TOKEN_QUERY_KEY = 'ark_token'

/**
 * Same-origin by default: in dev Vite proxies `/v1` and `/health` onto arkd,
 * in production the shell serves the page from the runtime itself.
 */
const BASE_URL = ((import.meta.env.VITE_ARK_BASE_URL as string | undefined) ?? '').replace(/\/$/, '')

export type Query = Record<string, string | number | boolean | null | undefined>

export class ApiError extends Error {
  readonly status: number
  readonly code: ArkErrorCode | 'unknown'
  readonly detail?: string

  constructor(args: { status: number; code: ArkErrorCode | 'unknown'; message: string; detail?: string }) {
    super(args.message)
    this.name = 'ApiError'
    this.status = args.status
    this.code = args.code
    this.detail = args.detail
  }

  /** Short operator-facing hint derived from the error code, never from prose. */
  get hint(): string {
    return this.code === 'unknown' ? `HTTP ${this.status}` : ERROR_HINTS[this.code]
  }
}

/**
 * Token source: `?ark_token=...` wins (handy for one-off deep links), otherwise
 * localStorage. Missing token means "no header", not "fail": the runtime decides.
 */
export function readToken(): string | null {
  try {
    const fromQuery = new URLSearchParams(window.location.search).get(TOKEN_QUERY_KEY)
    if (fromQuery) {
      window.localStorage.setItem(TOKEN_STORAGE_KEY, fromQuery)
      return fromQuery
    }
    return window.localStorage.getItem(TOKEN_STORAGE_KEY)
  } catch {
    return null
  }
}

export function buildUrl(path: string, query?: Query): string {
  const url = `${BASE_URL}${path}`
  if (!query) return url
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(query)) {
    if (value === null || value === undefined) continue
    params.set(key, String(value))
  }
  const qs = params.toString()
  return qs ? `${url}?${qs}` : url
}

type SendOptions = {
  body?: unknown
  query?: Query
  signal?: AbortSignal
  headers?: Record<string, string>
}

async function send(method: string, path: string, options: SendOptions = {}): Promise<Response> {
  const headers: Record<string, string> = { accept: 'application/json', ...options.headers }
  const token = readToken()
  if (token) headers.authorization = `Bearer ${token}`

  const init: RequestInit = { method, headers }
  if (options.signal) init.signal = options.signal
  if (options.body !== undefined) {
    headers['content-type'] = 'application/json'
    init.body = JSON.stringify(options.body)
  }

  const response = await fetch(buildUrl(path, options.query), init)
  if (!response.ok) throw await toApiError(response)
  return response
}

/** Decode the runtime's `{code, message, detail?}` body (design §6.2). */
export async function toApiError(response: Response): Promise<ApiError> {
  const fallback = `HTTP ${response.status} ${response.statusText}`.trim()
  let raw: unknown = null
  try {
    raw = await response.json()
  } catch {
    raw = null
  }
  if (raw && typeof raw === 'object') {
    const body = raw as { code?: unknown; message?: unknown; detail?: unknown }
    const code = typeof body.code === 'string' ? (body.code as ArkErrorCode) : 'unknown'
    const message = typeof body.message === 'string' ? body.message : fallback
    const detail = typeof body.detail === 'string' ? body.detail : undefined
    return new ApiError({ status: response.status, code, message, ...(detail ? { detail } : {}) })
  }
  return new ApiError({ status: response.status, code: 'unknown', message: fallback })
}

async function readJson<T>(response: Response): Promise<T> {
  if (response.status === 204) return undefined as T
  const text = await response.text()
  if (!text) return undefined as T
  return JSON.parse(text) as T
}

export async function get<T>(path: string, options: SendOptions = {}): Promise<T> {
  return readJson<T>(await send('GET', path, options))
}

export async function post<T>(path: string, body?: unknown, options: SendOptions = {}): Promise<T> {
  return readJson<T>(await send('POST', path, { ...options, body }))
}

export async function patch<T>(path: string, body?: unknown, options: SendOptions = {}): Promise<T> {
  return readJson<T>(await send('PATCH', path, { ...options, body }))
}

export async function del<T>(path: string, options: SendOptions = {}): Promise<T> {
  return readJson<T>(await send('DELETE', path, options))
}

export type SseFrame = {
  /** `id:` line — the runtime puts `seq` here. */
  id: string | null
  /** `event:` line — the runtime puts the event kind here. */
  event: string | null
  /** `data:` lines joined with `\n`. */
  data: string
}

const BLOCK_END = /\r?\n\r?\n/

/**
 * Hand-written `text/event-stream` reader. `EventSource` cannot send headers,
 * so the browser-native client is not an option for a token-authenticated stream.
 */
export async function* stream(path: string, options: SendOptions = {}): AsyncGenerator<SseFrame, void, void> {
  const response = await send('GET', path, {
    ...options,
    headers: { accept: 'text/event-stream', ...options.headers }
  })
  if (!response.body) {
    throw new ApiError({ status: response.status, code: 'internal_error', message: 'SSE 响应缺少 body' })
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  try {
    for (;;) {
      const { value, done } = await reader.read()
      if (done) return
      buffer += decoder.decode(value, { stream: true })

      for (;;) {
        const match = BLOCK_END.exec(buffer)
        if (!match || match.index === undefined) break
        const raw = buffer.slice(0, match.index)
        buffer = buffer.slice(match.index + match[0].length)
        const frame = parseSseFrame(raw)
        if (frame) yield frame
      }
    }
  } finally {
    void reader.cancel().catch(() => undefined)
  }
}

/** Parse one blank-line-delimited SSE block into a frame, skipping comments. */
export function parseSseFrame(raw: string): SseFrame | null {
  let id: string | null = null
  let event: string | null = null
  const data: string[] = []

  for (const line of raw.split(/\r?\n/)) {
    if (!line || line.startsWith(':')) continue
    const colon = line.indexOf(':')
    const field = colon === -1 ? line : line.slice(0, colon)
    const rawValue = colon === -1 ? '' : line.slice(colon + 1)
    const value = rawValue.startsWith(' ') ? rawValue.slice(1) : rawValue

    if (field === 'id') id = value
    else if (field === 'event') event = value
    else if (field === 'data') data.push(value)
    // `retry:` is accepted but ignored: backoff is owned by the stream client.
  }

  if (!event && data.length === 0) return null
  return { id, event, data: data.join('\n') }
}
