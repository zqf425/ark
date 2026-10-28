/**
 * Provider model-catalog form helpers.
 *
 * Kept outside the React component so JSON/header parsing and the rule that
 * remote metadata is applied only when explicit stay unit-testable.
 */
import type {
  ProviderModelCatalogRequest,
  ProviderModelSummary
} from '../../../shared/contracts'
import type { ProviderDraft } from './ProviderForm'

export type BuiltHeaders = { ok: true; headers: Record<string, string> } | { ok: false; error: string }

/** Parse the write-only custom headers field into a request-ready map. */
export function parseProviderHeaders(headersText: string): BuiltHeaders {
  const raw = headersText.trim()
  if (!raw) return { ok: true, headers: {} }
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { ok: false, error: 'headers 必须是 JSON 对象，例如 {"X-Trace":"1"}' }
    }
    const headers: Record<string, string> = {}
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      headers[key] = typeof value === 'string' ? value : String(value)
    }
    return { ok: true, headers }
  } catch {
    return { ok: false, error: 'headers 不是合法 JSON' }
  }
}

/** Apply only remote metadata that the catalog explicitly returned. */
export function applyCatalogModel(draft: ProviderDraft, model: ProviderModelSummary): ProviderDraft {
  return {
    ...draft,
    model: model.id,
    context_window_tokens: model.context_window_tokens?.toString() ?? '',
    supports_vision: model.supports_vision ?? null
  }
}

/** Build a model-catalog request without requiring the user to enter a model first. */
export function buildCatalogRequest(
  draft: ProviderDraft,
  editing: boolean
): { ok: true; request: ProviderModelCatalogRequest } | { ok: false; error: string } {
  const baseUrl = draft.base_url.trim()
  if (!baseUrl) return { ok: false, error: '查询模型前需要填写 base_url' }
  const headers = parseProviderHeaders(draft.headers_text)
  if (!headers.ok) return headers
  const apiKey = draft.api_key.trim()
  if (!editing && !apiKey) return { ok: false, error: '查询新供应商的模型前需要填写 api_key' }
  return {
    ok: true,
    request: {
      ...(editing && draft.id.trim() ? { provider_id: draft.id.trim() } : {}),
      connection: {
        base_url: baseUrl,
        ...(apiKey ? { api_key: apiKey } : {}),
        endpoint_format: draft.endpoint_format,
        headers: headers.headers
      }
    }
  }
}

/** Build a concise option label without representing unknown capabilities as false. */
export function catalogLabel(model: ProviderModelSummary): string {
  const details: string[] = []
  if (model.context_window_tokens) details.push(`${model.context_window_tokens} tokens`)
  if (model.max_output_tokens) details.push(`最大输出 ${model.max_output_tokens}`)
  if (model.supports_vision === true) details.push('支持识图')
  if (model.supports_vision === false) details.push('不支持识图')
  return [model.label ?? model.id, details.join(' · ')].filter(Boolean).join(' — ')
}
